//! How far each chat's work has got (`Progress`), so the person, an
//! orchestrator or the Brain dump reads it instead of asking the working
//! agent (a question is a steer: it interrupts the work and costs a turn).
//!
//! The chat's host engine keeps one [`Tracker`] per chat it runs, fed from
//! what it already knows: the session row it publishes (status, activity,
//! todo list), and the reports its crew threads file. The deterministic
//! fields are always on and cost nothing. The `note` is one sentence from a
//! cheap one-shot model on the title path (no tools, quoted data:
//! `zeron_harness::PROGRESS_INSTRUCTIONS`), written when a todo step changes,
//! a report is filed, a turn ends, every [`NOTE_PERIODIC`] while working when
//! the transcript changed, and on demand (`progressCheck`). Automatic notes
//! come at most every [`NOTE_AUTO_EVERY`] per chat, on-demand ones every
//! [`NOTE_ON_DEMAND_EVERY`] (the latest note otherwise), and
//! `ZERON_PROGRESS_NOTES=0` turns the automatic ones off. Nothing here ever
//! messages or steers the chat.
//!
//! Rows are the synced registry kind `progress`, written by the host only,
//! only on change (a step alone at most every
//! [`PROGRESS_STEP_WRITE_SECS`]). `WatchProgress` streams them ([`stream`]).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Duration as TimeDelta, Utc};
use futures::FutureExt;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use zeron_doc::{MessagePart, MessageRole, SessionMessageEntry};
use zeron_proto::orchestration::{
    ActivityKind, ChatActivity, PROGRESS_STEP_WRITE_SECS, PROGRESS_STRETCH_GAP_SECS, PROGRESS_TOOL_STALL_SECS, Progress, ProgressState,
    ProgressSteps, ReportState, TodoProgress, activity_of, live_label, progress_note,
};
use zeron_proto::{DoneStatus, HarnessId, Session, SessionStatus, ToolCall};
use zeron_rpc::RpcError;

use super::Orchestration;
use super::ops::Who;
use crate::rpc::EngineRpc;

/// The least a working chat goes without a step before it counts as stalled.
pub(crate) const STALL_FLOOR: TimeDelta = TimeDelta::minutes(5);
/// One tool call running longer than this counts as stalled.
pub(crate) const TOOL_STALL: TimeDelta = TimeDelta::seconds(PROGRESS_TOOL_STALL_SECS);
/// Automatic notes, at most this often per chat.
pub(crate) const NOTE_AUTO_EVERY: TimeDelta = TimeDelta::seconds(60);
/// On-demand notes, at most this often per chat (the latest note otherwise).
pub(crate) const NOTE_ON_DEMAND_EVERY: TimeDelta = TimeDelta::seconds(15);
/// A working chat's note is made again this often, when its transcript
/// changed.
pub(crate) const NOTE_PERIODIC: TimeDelta = TimeDelta::minutes(5);
/// Finished chats stay in `WatchProgress` this long (crew threads always).
pub(crate) const RECENT: TimeDelta = TimeDelta::hours(24);
/// How often the stall clock and periodic notes are looked at.
const TICK: std::time::Duration = std::time::Duration::from_secs(15);
/// How much of the transcript's tail a note is made from.
pub(crate) const TAIL_MAX_CHARS: usize = 4000;
/// Step intervals and item times kept (the medians' window).
const KEPT: usize = 32;
/// A non-crew chat's request, as a note's input keeps it.
const GOAL_MAX_CHARS: usize = 600;
/// The longest `progressCheck` waits for a fresh note before it replies
/// with the latest (`refreshing`).
const ON_DEMAND_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
/// Turns off automatic notes when `0`, `off` or `false`.
pub const NOTES_ENV: &str = "ZERON_PROGRESS_NOTES";

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn median(values: &VecDeque<i64>) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted: Vec<i64> = values.iter().copied().collect();
    sorted.sort_unstable();
    let mid = sorted.len() / 2;
    Some(if sorted.len() % 2 == 0 { (sorted[mid - 1] + sorted[mid]) / 2 } else { sorted[mid] })
}

fn keep(values: &mut VecDeque<i64>, value: i64) {
    values.push_back(value.max(0));
    while values.len() > KEPT {
        values.pop_front();
    }
}

// ── The deterministic part ──────────────────────────────────────────────────

/// What one observation of a chat's session row said, for the note
/// triggers.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Observed {
    /// A turn just ended (working → not, the person's question aside).
    pub turn_ended: bool,
    /// A todo item started or completed.
    pub todo_step: bool,
}

/// One chat's progress as its host follows it. Pure: every input carries
/// its time.
#[derive(Debug, Clone)]
pub(crate) struct Tracker {
    chat_id: String,
    device_id: String,
    status: Option<SessionStatus>,
    waiting: bool,
    outcome: Option<DoneStatus>,
    started_at: Option<DateTime<Utc>>,
    /// When it last started working (a stretch's start, or a resume after a
    /// wait): the stall clock never runs from before it.
    resumed_at: Option<DateTime<Utc>>,
    /// When its last turn ended.
    ended_at: Option<DateTime<Utc>>,
    last_step_at: Option<DateTime<Utc>>,
    /// Between steps, ms.
    intervals: VecDeque<i64>,
    /// Per finished todo item in this stretch, ms.
    item_times: VecDeque<i64>,
    /// When the todo list's done count last went up (or the list began).
    done_at: Option<DateTime<Utc>>,
    todo: Option<TodoProgress>,
    activity: Option<ChatActivity>,
    reported_done: bool,
    note: Option<String>,
    note_at: Option<DateTime<Utc>>,
}

impl Tracker {
    pub(crate) fn new(chat_id: &str, device_id: &str) -> Self {
        Self {
            chat_id: chat_id.to_owned(),
            device_id: device_id.to_owned(),
            status: None,
            waiting: false,
            outcome: None,
            started_at: None,
            resumed_at: None,
            ended_at: None,
            last_step_at: None,
            intervals: VecDeque::new(),
            item_times: VecDeque::new(),
            done_at: None,
            todo: None,
            activity: None,
            reported_done: false,
            note: None,
            note_at: None,
        }
    }

    /// Picks up where this host's own row left off (a restart).
    pub(crate) fn resume_from(&mut self, row: &Progress) {
        self.started_at = row.started_at;
        self.last_step_at = row.last_step_at;
        self.note = row.note.clone();
        self.note_at = row.note_at;
        self.reported_done = row.state == ProgressState::Done;
        self.ended_at = Some(row.updated_at);
        if let Some(steps) = row.steps {
            self.todo = Some(TodoProgress { done: steps.done, total: steps.total, current: None });
        }
    }

    fn step(&mut self, now: DateTime<Utc>) {
        if let Some(from) = self.last_step_at.max(self.resumed_at) {
            keep(&mut self.intervals, (now - from).num_milliseconds());
        }
        self.last_step_at = Some(now);
    }

    /// Whether a turn starting now goes on with the current stretch.
    fn continues_stretch(&self, now: DateTime<Utc>) -> bool {
        if self.started_at.is_none() {
            return false;
        }
        match self.status {
            Some(SessionStatus::Working | SessionStatus::AwaitingInput) => true,
            _ if self.waiting => true,
            _ => {
                let finished = self.reported_done
                    || self.todo.as_ref().is_some_and(|t| t.total > 0 && t.done >= t.total);
                let recent = self
                    .ended_at
                    .is_some_and(|at| now - at <= TimeDelta::seconds(PROGRESS_STRETCH_GAP_SECS));
                recent && !finished
            }
        }
    }

    /// Take in the chat's session row as its host publishes it now.
    pub(crate) fn observe(&mut self, session: &Session, now: DateTime<Utc>) -> Observed {
        let mut seen = Observed::default();
        let was_working = self.status == Some(SessionStatus::Working);
        let working = session.status == SessionStatus::Working;
        if working && !was_working {
            if !self.continues_stretch(now) {
                // A new stretch of work.
                self.started_at = Some(session.started_at.filter(|at| *at <= now).unwrap_or(now));
                self.last_step_at = None;
                self.item_times.clear();
                self.done_at = self.started_at;
                self.reported_done = false;
                // A finished list belongs to the last stretch.
                if self.todo.as_ref().is_some_and(|t| t.done >= t.total) {
                    self.todo = None;
                }
            }
            self.resumed_at = Some(now);
        }
        if was_working && !working && session.status != SessionStatus::AwaitingInput {
            self.ended_at = Some(now);
            seen.turn_ended = true;
        }
        // Todo: an item done or started is a step.
        if session.todo != self.todo {
            let before = self.todo.clone();
            let after = session.todo.clone();
            match (&before, &after) {
                (Some(b), Some(a)) if a.done >= b.done && (a.total == b.total || a.done > b.done) => {
                    let finished = a.done - b.done;
                    if finished > 0 {
                        if let Some(from) = self.done_at {
                            let each = (now - from).num_milliseconds() / finished as i64;
                            for _ in 0..finished {
                                keep(&mut self.item_times, each);
                            }
                        }
                        self.done_at = Some(now);
                    }
                    if finished > 0 || a.current != b.current {
                        seen.todo_step = true;
                    }
                }
                (_, Some(_)) => {
                    // A new list, or one started over.
                    self.item_times.clear();
                    self.done_at = Some(now);
                    seen.todo_step = true;
                }
                (_, None) => {}
            }
            if seen.todo_step {
                self.step(now);
            }
            self.todo = after;
        }
        // A tool that was running and isn't any more finished: a step.
        if working {
            let was_tool = self.activity.as_ref().is_some_and(|a| a.kind != ActivityKind::Thinking);
            let changed = match (&self.activity, &session.activity) {
                (Some(a), Some(b)) => a.kind != b.kind || a.label != b.label,
                (None, None) => false,
                _ => true,
            };
            if changed && was_tool && !seen.todo_step {
                self.step(now);
            }
            self.activity = session.activity.clone();
        } else {
            self.activity = None;
        }
        self.status = Some(session.status);
        self.waiting = session.waiting.is_some();
        self.outcome = session.last_turn_outcome.map(|o| o.status);
        seen
    }

    /// The chat filed a report.
    pub(crate) fn report(&mut self, state: ReportState, now: DateTime<Utc>) {
        self.step(now);
        match state {
            ReportState::Done => self.reported_done = true,
            ReportState::Progress | ReportState::Message => {}
            _ => self.reported_done = false,
        }
    }

    /// A step seen before this engine followed the chat (a proxy's first
    /// read of the transcript), when it knows of none since the stretch
    /// began.
    pub(crate) fn seed_step(&mut self, at: DateTime<Utc>) {
        if self.last_step_at.is_none() && self.started_at.is_none_or(|start| at >= start) {
            self.last_step_at = Some(at);
        }
    }

    pub(crate) fn set_note(&mut self, note: String, at: DateTime<Utc>) {
        self.note = Some(note);
        self.note_at = Some(at);
    }

    pub(crate) fn started_at(&self) -> Option<DateTime<Utc>> {
        self.started_at
    }

    pub(crate) fn state(&self) -> ProgressState {
        match self.status {
            Some(SessionStatus::Working) => ProgressState::Working,
            Some(SessionStatus::AwaitingInput) => ProgressState::AwaitingInput,
            Some(SessionStatus::Errored) => ProgressState::Failed,
            _ if self.waiting => ProgressState::Waiting,
            _ if self.reported_done => ProgressState::Done,
            None => ProgressState::Idle,
            Some(SessionStatus::Idle) => match self.outcome {
                Some(DoneStatus::Errored) => ProgressState::Failed,
                Some(DoneStatus::Interrupted) => ProgressState::Idle,
                _ if self.todo.as_ref().is_some_and(|t| t.done < t.total) => ProgressState::Idle,
                Some(DoneStatus::Completed) => ProgressState::Done,
                None => ProgressState::Idle,
            },
        }
    }

    /// The tool call running now: its label and when it started.
    pub(crate) fn running_tool(&self, now: DateTime<Utc>) -> Option<(&str, DateTime<Utc>)> {
        if self.state() != ProgressState::Working {
            return None;
        }
        self.activity
            .as_ref()
            .filter(|a| a.kind != ActivityKind::Thinking)
            .map(|a| (a.label.as_str(), a.since.min(now)))
    }

    /// Working and seemingly stuck. No tool call running: no step for
    /// longer than max(5 min, 3 × the median step interval). One running
    /// (a long test run is healthy): only past [`TOOL_STALL`].
    pub(crate) fn stalled(&self, now: DateTime<Utc>) -> bool {
        if self.state() != ProgressState::Working {
            return false;
        }
        if let Some((_, since)) = self.running_tool(now) {
            return now - since > TOOL_STALL;
        }
        let Some(from) = self.last_step_at.max(self.resumed_at).or(self.started_at) else {
            return false;
        };
        let limit = median(&self.intervals)
            .map(|m| TimeDelta::milliseconds(m.saturating_mul(3)))
            .unwrap_or(STALL_FLOOR)
            .max(STALL_FLOOR);
        now - from > limit
    }

    /// Seconds left as of `now`: the median item time × the items left,
    /// less what the current item already took (never below the items after
    /// it). Only while working, with ≥ 3 items done and timed of a known
    /// total.
    pub(crate) fn eta_secs(&self, now: DateTime<Utc>) -> Option<u32> {
        if self.state() != ProgressState::Working {
            return None;
        }
        let todo = self.todo.as_ref()?;
        if todo.done < 3 || todo.done >= todo.total || self.item_times.len() < 3 {
            return None;
        }
        let median = median(&self.item_times)?;
        let left = (todo.total - todo.done) as i64;
        let into = self.done_at.map_or(0, |at| (now - at).num_milliseconds().max(0));
        let ms = (median * left - into).max(median * (left - 1));
        Some(((ms + 999) / 1000).clamp(0, u32::MAX as i64) as u32)
    }

    pub(crate) fn snapshot(&self, now: DateTime<Utc>) -> Progress {
        let state = self.state();
        // A call shows once it has run a while: short ones would only churn
        // the row (the session row carries the live activity).
        let tool = self
            .running_tool(now)
            .filter(|(_, since)| now - *since >= TimeDelta::seconds(PROGRESS_STEP_WRITE_SECS));
        let step = self
            .todo
            .as_ref()
            .and_then(|t| t.current.clone())
            .or_else(|| tool.map(|(label, _)| label.to_owned()))
            .map(|s| live_label(&s))
            .filter(|s| !s.is_empty());
        Progress {
            chat_id: self.chat_id.clone(),
            device_id: self.device_id.clone(),
            state,
            started_at: self.started_at,
            last_step_at: self.last_step_at,
            step,
            steps: self.todo.as_ref().map(|t| ProgressSteps { done: t.done, total: t.total }),
            note: self.note.clone(),
            note_at: self.note_at,
            stalled: self.stalled(now),
            tool: tool.map(|(label, _)| live_label(label)).filter(|l| !l.is_empty()),
            tool_since: tool.map(|(_, since)| since),
            refreshing: false,
            proxy: false,
            eta_secs: self.eta_secs(now),
            updated_at: now,
        }
    }
}

/// Whether `next` says something `written` doesn't: any field but the
/// write time, an ETA whose end moved by under a minute, or a step time
/// newer by under [`PROGRESS_STEP_WRITE_SECS`] with nothing else changed.
pub(crate) fn needs_write(written: Option<&Progress>, next: &Progress) -> bool {
    let Some(old) = written else { return true };
    let eta_end = |p: &Progress| p.eta_secs.map(|e| p.updated_at + TimeDelta::seconds(e as i64));
    let eta_moved = match (eta_end(old), eta_end(next)) {
        (Some(a), Some(b)) => (a - b).num_seconds().abs() >= 60,
        (None, None) => false,
        _ => true,
    };
    let rest_changed = old.chat_id != next.chat_id
        || old.device_id != next.device_id
        || old.state != next.state
        || old.started_at != next.started_at
        || old.step != next.step
        || old.steps != next.steps
        || old.note != next.note
        || old.note_at != next.note_at
        || old.stalled != next.stalled
        || old.tool != next.tool
        || old.tool_since != next.tool_since
        || old.refreshing != next.refreshing
        || eta_moved;
    let step_moved = match (old.last_step_at, next.last_step_at) {
        (Some(a), Some(b)) => (b - a).num_seconds().abs() >= PROGRESS_STEP_WRITE_SECS,
        (None, None) => false,
        _ => true,
    };
    rest_changed || step_moved
}

// ── The note ────────────────────────────────────────────────────────────────

/// Why a note is wanted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Why {
    TodoStep,
    Report,
    TurnEnded,
    Periodic,
    OnDemand,
}

impl Why {
    fn automatic(self) -> bool {
        self != Why::OnDemand
    }
}

/// Whether a note may be made now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Claim {
    Go,
    /// Not now: the latest note stands.
    Latest,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct NoteClock {
    last_try: Option<DateTime<Utc>>,
    in_flight: bool,
    /// An automatic note held back by the limit, made once it allows.
    owed: bool,
    /// The transcript the last note was made from.
    fingerprint: Option<String>,
}

impl NoteClock {
    /// The rate limits: automatic notes ≥ [`NOTE_AUTO_EVERY`] apart (one
    /// held back is owed), on-demand ≥ [`NOTE_ON_DEMAND_EVERY`]; never two at
    /// once.
    pub(crate) fn claim(&mut self, why: Why, auto_on: bool, now: DateTime<Utc>) -> Claim {
        if why.automatic() && !auto_on {
            return Claim::Latest;
        }
        let every = if why.automatic() { NOTE_AUTO_EVERY } else { NOTE_ON_DEMAND_EVERY };
        let soon = self.last_try.is_some_and(|at| now - at < every);
        if self.in_flight || soon {
            if why.automatic() && why != Why::Periodic {
                self.owed = true;
            }
            return Claim::Latest;
        }
        self.in_flight = true;
        self.owed = false;
        self.last_try = Some(now);
        Claim::Go
    }

    fn done(&mut self, fingerprint: Option<String>) {
        self.in_flight = false;
        if fingerprint.is_some() {
            self.fingerprint = fingerprint;
        }
    }
}

/// A one-shot note run: the chat's harness and the whole prompt → the raw
/// reply. The engine's goes through the title path; tests swap it.
pub(crate) type NoteFn = Arc<dyn Fn(HarnessId, String) -> BoxFuture<'static, Option<String>> + Send + Sync>;

/// What the transcript looks like, to tell whether it changed.
pub(crate) fn fingerprint(entries: &[SessionMessageEntry]) -> String {
    let Some(last) = entries.last() else { return "0".into() };
    let size: usize = last
        .parts
        .iter()
        .map(|p| match p {
            MessagePart::Text { text, .. } | MessagePart::Reasoning { text, .. } => text.len(),
            MessagePart::Tool { resolved, .. } => 1 + *resolved as usize,
            _ => 1,
        })
        .sum();
    format!("{}:{}:{}:{}:{:?}", entries.len(), last.id, last.parts.len(), size, last.status)
}

fn redacted(text: &str, max: usize) -> String {
    zeron_proto::orchestration::prompt_text::one_line(&zeron_proto::redact::redact_command_output(text), max)
}

fn entry_text(entry: &SessionMessageEntry) -> String {
    entry
        .parts
        .iter()
        .filter_map(|p| match p {
            MessagePart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The newest transcript entries, one compact redacted line each (texts,
/// tool calls by label), oldest first, at most `max` chars in all.
pub(crate) fn transcript_tail(entries: &[SessionMessageEntry], max: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut used = 0usize;
    'entries: for entry in entries.iter().rev() {
        let who = match entry.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "agent",
            MessageRole::System => continue,
        };
        let mut mine: Vec<String> = Vec::new();
        for part in &entry.parts {
            let line = match part {
                MessagePart::Text { text, .. } if !text.trim().is_empty() => format!("{who}: {}", redacted(text, 400)),
                MessagePart::Tool { call, is_error, resolved, .. } => {
                    let (_, label) = activity_of(call);
                    let state = match (resolved, is_error) {
                        (_, true) => " (failed)",
                        (false, _) => " (running)",
                        _ => "",
                    };
                    format!("tool: {}{state}", redacted(&label, 100))
                }
                _ => continue,
            };
            mine.push(line);
        }
        for line in mine.into_iter().rev() {
            let cost = line.chars().count() + 1;
            if used + cost > max {
                // What room is left takes the start of this line.
                let room = max.saturating_sub(used + 2);
                if room >= 40 {
                    let mut cut: String = line.chars().take(room).collect();
                    cut.push('\u{2026}');
                    lines.push(cut);
                }
                break 'entries;
            }
            used += cost;
            lines.push(line);
        }
    }
    lines.reverse();
    lines.join("\n")
}

/// The newest todo list in the transcript: `(text, status)` per item.
pub(crate) fn todo_items(entries: &[SessionMessageEntry]) -> Vec<(String, &'static str)> {
    let list = entries.iter().rev().flat_map(|e| e.parts.iter().rev()).find_map(|p| match p {
        MessagePart::Tool { call: ToolCall::Todo { items }, .. } => Some(items),
        _ => None,
    });
    list.map(|items| {
        items
            .iter()
            .take(40)
            .map(|item| {
                let status = match item.status() {
                    zeron_proto::TodoStatus::Completed => "done",
                    zeron_proto::TodoStatus::InProgress => "in progress",
                    zeron_proto::TodoStatus::Pending => "pending",
                };
                (live_label(&item.text), status)
            })
            .collect()
    })
    .unwrap_or_default()
}

/// The request a stretch works on: the person's (or sender's) message that
/// started it, else the chat's first.
pub(crate) fn request_of(entries: &[SessionMessageEntry], started_at: Option<DateTime<Utc>>) -> String {
    let users = || entries.iter().filter(|e| e.role == MessageRole::User);
    let started = started_at.map(|at| at.timestamp_millis() + 60_000);
    let entry = started
        .and_then(|cut| users().filter(|e| e.created_at <= cut).last())
        .or_else(|| users().next());
    entry.map(|e| redacted(&entry_text(e), GOAL_MAX_CHARS)).unwrap_or_default()
}

/// The whole prompt a note is made from: the instructions, then everything
/// about the chat as one JSON document (quoted data, never instructions).
pub(crate) fn note_prompt(
    request: &str,
    item: Option<&str>,
    todo: &[(String, &'static str)],
    tail: &str,
    progress: &Progress,
    now: DateTime<Utc>,
) -> String {
    let data = json!({
        "request": request,
        "boardItem": item,
        "todo": todo.iter().map(|(text, status)| json!({ "text": text, "status": status })).collect::<Vec<_>>(),
        "status": {
            "state": progress.state,
            "elapsed": progress.started_at.map(|at| zeron_proto::orchestration::short_duration((now - at).num_seconds())),
            "sinceLastStep": progress.last_step_at.map(|at| zeron_proto::orchestration::short_duration((now - at).num_seconds())),
            "steps": progress.steps,
            "currentStep": progress.step,
            "stalled": progress.stalled,
        },
        "recentTranscript": tail,
    });
    format!(
        "{}\n\nChat data (JSON):\n{}",
        zeron_harness::PROGRESS_INSTRUCTIONS,
        serde_json::to_string_pretty(&data).unwrap_or_default()
    )
}

// ── The book: every chat this host runs ─────────────────────────────────────

#[derive(Debug, Clone)]
struct Entry {
    tracker: Tracker,
    written: Option<Progress>,
    clock: NoteClock,
}

/// Every chat this engine hosts that has worked since it started, and the
/// rows it wrote for them.
pub(crate) struct ProgressBook {
    chats: Mutex<HashMap<String, Entry>>,
    auto_notes: std::sync::atomic::AtomicBool,
    note_fn: Mutex<Option<NoteFn>>,
    proxies: proxy::Proxies,
}

impl Default for ProgressBook {
    fn default() -> Self {
        let off = std::env::var(NOTES_ENV)
            .is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "off" | "false" | "no"));
        Self {
            chats: Mutex::new(HashMap::new()),
            auto_notes: std::sync::atomic::AtomicBool::new(!off),
            note_fn: Mutex::new(None),
            proxies: proxy::Proxies::default(),
        }
    }
}

/// Zeron's own chats that never list as sessions: no progress.
fn untracked(chat_id: &str) -> bool {
    zeron_proto::voice::is_orchestrator_chat(chat_id) || zeron_proto::orchestration::is_brain_dump_chat(chat_id)
}

impl ProgressBook {
    /// Automatic notes on or off (the deterministic fields are always on).
    pub(crate) fn set_auto_notes(&self, on: bool) {
        self.auto_notes.store(on, std::sync::atomic::Ordering::Release);
    }

    fn auto_notes(&self) -> bool {
        self.auto_notes.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Make notes with `note_fn` instead of the title path (tests).
    #[cfg(test)]
    pub(crate) fn set_note_fn(&self, note_fn: NoteFn) {
        *lock(&self.note_fn) = Some(note_fn);
    }

    fn entry<'a>(chats: &'a mut HashMap<String, Entry>, orch: &Orchestration, chat_id: &str) -> &'a mut Entry {
        chats.entry(chat_id.to_owned()).or_insert_with(|| {
            let mut tracker = Tracker::new(chat_id, orch.device_id());
            let row = orch
                .workspace()
                .read_orchestration_with(|doc| doc.progress(chat_id))
                .filter(|row| row.device_id == orch.device_id());
            if let Some(row) = &row {
                tracker.resume_from(row);
            }
            Entry { tracker, written: row, clock: NoteClock::default() }
        })
    }

    /// Write `chat_id`'s row when it says something new. The row as it
    /// stands now (fresher than the written one).
    fn flush(&self, orch: &Orchestration, chat_id: &str, now: DateTime<Utc>) -> Option<Progress> {
        let (next, write) = {
            let mut chats = lock(&self.chats);
            let entry = chats.get_mut(chat_id)?;
            let mut next = entry.tracker.snapshot(now);
            next.refreshing = entry.clock.in_flight;
            let write = needs_write(entry.written.as_ref(), &next);
            if write {
                entry.written = Some(next.clone());
            }
            (next, write)
        };
        if write {
            orch.workspace().write_orchestration(|doc| doc.upsert_progress(&next));
        }
        Some(next)
    }

    /// Take in a local session row; the note it calls for, if any.
    pub(crate) fn observe(&self, orch: &Orchestration, session: &Session, now: DateTime<Utc>) -> Option<Why> {
        if untracked(&session.chat_id) {
            return None;
        }
        let seen = {
            let mut chats = lock(&self.chats);
            // A chat is followed from when it works (or from its own row,
            // after a restart): an idle one never gets a row.
            if !chats.contains_key(&session.chat_id) {
                let active = matches!(session.status, SessionStatus::Working | SessionStatus::AwaitingInput)
                    || session.waiting.is_some();
                let own_row = || {
                    orch.workspace()
                        .read_orchestration_with(|doc| doc.progress(&session.chat_id))
                        .is_some_and(|row| row.device_id == orch.device_id())
                };
                if !active && !own_row() {
                    return None;
                }
            }
            Self::entry(&mut chats, orch, &session.chat_id).tracker.observe(session, now)
        };
        self.flush(orch, &session.chat_id, now);
        if seen.turn_ended {
            Some(Why::TurnEnded)
        } else if seen.todo_step {
            Some(Why::TodoStep)
        } else {
            None
        }
    }

    /// A chat hosted here filed a report.
    pub(crate) fn on_report(&self, orch: &Orchestration, chat_id: &str, state: ReportState, now: DateTime<Utc>) {
        if untracked(chat_id) {
            return;
        }
        {
            let mut chats = lock(&self.chats);
            Self::entry(&mut chats, orch, chat_id).tracker.report(state, now);
        }
        self.flush(orch, chat_id, now);
        spawn_note(orch.clone(), chat_id.to_owned(), Why::Report);
    }

    /// Every tracked chat's stall clock and periodic note, now. The chats
    /// owed a note.
    fn tick(&self, orch: &Orchestration, now: DateTime<Utc>) -> Vec<(String, Why)> {
        let ids: Vec<String> = lock(&self.chats).keys().cloned().collect();
        let mut due = Vec::new();
        for id in ids {
            self.flush(orch, &id, now);
            let chats = lock(&self.chats);
            let Some(entry) = chats.get(&id) else { continue };
            let working = entry.tracker.state() == ProgressState::Working;
            let clock = &entry.clock;
            if clock.in_flight {
                continue;
            }
            if clock.owed && clock.last_try.is_none_or(|at| now - at >= NOTE_AUTO_EVERY) {
                due.push((id.clone(), Why::TurnEnded));
            } else if working {
                let since = clock.last_try.or(entry.tracker.started_at());
                if since.is_some_and(|at| now - at >= NOTE_PERIODIC) {
                    due.push((id.clone(), Why::Periodic));
                }
            }
        }
        due
    }

    /// The row as it stands now, for a chat hosted here (`None`: it hasn't
    /// worked here, and has no row of this host's).
    pub(crate) fn current(&self, orch: &Orchestration, chat_id: &str, now: DateTime<Utc>) -> Option<Progress> {
        if !lock(&self.chats).contains_key(chat_id) {
            let row = orch
                .workspace()
                .read_orchestration_with(|doc| doc.progress(chat_id))
                .filter(|row| row.device_id == orch.device_id());
            let session = orch
                .rpc()
                .and_then(|rpc| rpc.sessions_ref().watch_sessions().borrow().iter().find(|s| s.chat_id == chat_id).cloned());
            if row.is_none() && session.is_none() {
                return None;
            }
            let mut chats = lock(&self.chats);
            let entry = Self::entry(&mut chats, orch, chat_id);
            if let Some(session) = &session {
                entry.tracker.observe(session, now);
            }
        }
        self.flush(orch, chat_id, now)
    }

    fn claim(&self, chat_id: &str, why: Why, now: DateTime<Utc>) -> Claim {
        let auto = self.auto_notes();
        let mut chats = lock(&self.chats);
        match chats.get_mut(chat_id) {
            Some(entry) => entry.clock.claim(why, auto, now),
            None => Claim::Latest,
        }
    }

    fn release(&self, chat_id: &str, fingerprint: Option<String>) {
        if let Some(entry) = lock(&self.chats).get_mut(chat_id) {
            entry.clock.done(fingerprint);
        }
    }

    /// The cheap one-shot run (a chat's harness and a whole prompt → its
    /// raw reply): the title path, or what a test put in its place.
    pub(crate) fn one_shot(&self, rpc: &EngineRpc) -> NoteFn {
        lock(&self.note_fn).clone().unwrap_or_else(|| title_path(rpc.registry_ref().clone()))
    }

    fn last_fingerprint(&self, chat_id: &str) -> Option<String> {
        lock(&self.chats).get(chat_id).and_then(|e| e.clock.fingerprint.clone())
    }

    /// Make `chat_id`'s note now when the limits allow it; the row after.
    pub(crate) async fn note(&self, orch: &Orchestration, chat_id: &str, why: Why) -> Option<Progress> {
        let now = Utc::now();
        if self.claim(chat_id, why, now) == Claim::Go {
            // The row says a note is on its way while it is.
            self.flush(orch, chat_id, now);
            let made = self.make_note(orch, chat_id, why).await;
            let note = made.as_ref().and_then(|(note, fp)| note.clone().map(|n| (n, fp.clone())));
            self.release(chat_id, note.as_ref().map(|(_, fp)| fp.clone()));
            if let Some((note, _)) = note
                && let Some(entry) = lock(&self.chats).get_mut(chat_id)
            {
                entry.tracker.set_note(note, Utc::now());
            }
        }
        self.flush(orch, chat_id, Utc::now())
    }

    /// The note's run. `None` when it couldn't read the chat; `(None, fp)`
    /// when the model gave nothing or the transcript hasn't changed since
    /// the last note.
    async fn make_note(&self, orch: &Orchestration, chat_id: &str, why: Why) -> Option<(Option<String>, String)> {
        let rpc = orch.rpc()?;
        let entries = rpc.doc_host_ref().open(chat_id).ok()?.doc().read_entries().ok()?;
        let fp = fingerprint(&entries);
        if matches!(why, Why::Periodic | Why::OnDemand) && self.last_fingerprint(chat_id).as_deref() == Some(fp.as_str()) {
            return Some((None, fp));
        }
        let now = Utc::now();
        let (progress, started) = {
            let chats = lock(&self.chats);
            let entry = chats.get(chat_id)?;
            (entry.tracker.snapshot(now), entry.tracker.started_at())
        };
        let note = self.note_from(orch, &rpc, chat_id, &entries, &progress, started, now).await;
        if note.is_none() {
            tracing::debug!(chat = %chat_id, ?why, "progress note: nothing written");
        }
        Some((note, fp))
    }

    /// A note for `chat_id` from its transcript `entries` and `progress`:
    /// the prompt (quoted data) through the cheap one-shot run on the chat's
    /// harness, cleaned.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn note_from(
        &self,
        orch: &Orchestration,
        rpc: &EngineRpc,
        chat_id: &str,
        entries: &[SessionMessageEntry],
        progress: &Progress,
        started: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Option<String> {
        let item = orch
            .crew_of(chat_id)
            .and_then(|m| m.item_id)
            .and_then(|id| orch.item(&id))
            .map(|i| redacted(&format!("{}: {}", i.title, i.detail), GOAL_MAX_CHARS));
        let prompt = note_prompt(
            &request_of(entries, started),
            item.as_deref(),
            &todo_items(entries),
            &transcript_tail(entries, TAIL_MAX_CHARS),
            progress,
            now,
        );
        let harness = rpc
            .workspace_host()
            .chat(chat_id)
            .ok()
            .flatten()
            .and_then(|c| c.config.map(|c| c.harness))
            .unwrap_or_else(|| rpc.doc_host_ref().default_harness());
        let raw = self.one_shot(rpc)(harness, prompt).await;
        raw.map(|r| progress_note(&r)).filter(|n| !n.is_empty())
    }

    /// Chats on hosts that predate progress, worked out here.
    pub(crate) fn proxies(&self) -> &proxy::Proxies {
        &self.proxies
    }
}

/// One-shot runs through the title path, on the chat's harness's cheapest
/// model: the raw reply (each caller cleans its own).
fn title_path(registry: Arc<crate::registry::HarnessRegistry>) -> NoteFn {
    Arc::new(move |harness, prompt| {
        let registry = registry.clone();
        async move {
            let (harness, model) = crate::titles::side_run_harness(&registry, harness)?;
            crate::titles::one_shot(&registry, harness, model, prompt, |raw| {
                Some(raw.trim().to_owned()).filter(|r| !r.is_empty())
            })
            .await
        }
        .boxed()
    })
}

fn spawn_note(orch: Orchestration, chat_id: String, why: Why) {
    tokio::spawn(async move {
        orch.progress().note(&orch, &chat_id, why).await;
    });
}

/// Follow this engine's chats: their session rows, the stall clock and
/// periodic notes.
pub(crate) fn start(orch: Orchestration) {
    tokio::spawn(async move {
        let mut sessions_rx = loop {
            match orch.rpc() {
                Some(rpc) => break rpc.sessions_ref().watch_sessions(),
                None => tokio::time::sleep(std::time::Duration::from_secs(1)).await,
            }
        };
        // A `ProgressCheck` from another device for a chat hosted here.
        if let Some(rpc) = orch.rpc() {
            let checks = orch.clone();
            rpc.doc_host_ref().set_progress_checks(Arc::new(move |chat: &str| {
                spawn_note(checks.clone(), chat.to_owned(), Why::OnDemand);
            }));
        }
        proxy::start(orch.clone());
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let device = orch.device_id().to_owned();
        loop {
            tokio::select! {
                changed = sessions_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let sessions: Vec<Session> = sessions_rx.borrow_and_update().clone();
                    let now = Utc::now();
                    for session in sessions.iter().filter(|s| s.device_id == device) {
                        if let Some(why) = orch.progress().observe(&orch, session, now) {
                            spawn_note(orch.clone(), session.chat_id.clone(), why);
                        }
                    }
                }
                _ = tick.tick() => {
                    for (chat, why) in orch.progress().tick(&orch, Utc::now()) {
                        spawn_note(orch.clone(), chat, why);
                    }
                }
            }
        }
    });
}

// ── Reading it: progressCheck and WatchProgress ─────────────────────────────

/// The chats an agent may read the progress of: its own, the chats it
/// started, an orchestrator's crew; the Brain dump any.
pub(crate) fn may_check(rpc: &EngineRpc, who: &Who, chat_id: &str) -> bool {
    let Who::Agent { chat, role } = who else { return true };
    if chat == chat_id || matches!(role, super::ChatRole::BrainDump { .. }) {
        return true;
    }
    if let super::ChatRole::Orchestrator { id } = role
        && rpc
            .orchestration()
            .workspace()
            .read_orchestration_with(|doc| doc.crew_member(chat_id))
            .is_some_and(|m| m.orchestrator_id == *id)
    {
        return true;
    }
    rpc.workspace_host()
        .chat(chat_id)
        .ok()
        .flatten()
        .is_some_and(|c| c.parent_chat_id.as_deref() == Some(chat.as_str()))
}

fn reply(progress: &Progress, refreshing: bool) -> Value {
    let progress = Progress { refreshing: progress.refreshing || refreshing, ..progress.clone() };
    let mut value = serde_json::to_value(progress).unwrap_or(Value::Null);
    // Always said, so a caller needn't know the field's default.
    if let Value::Object(map) = &mut value {
        map.entry("refreshing").or_insert(json!(false));
    }
    value
}

/// `chat_id`'s progress with its note made fresh, and whether that note is
/// still on its way. On its host the note is made now (the limits allow);
/// another device's chat is asked through its doc (`ProgressCheck`), and
/// the reply is the synced row until the fresh note arrives.
pub(crate) async fn check(rpc: &EngineRpc, chat_id: &str) -> Result<(Progress, bool), RpcError> {
    let nothing = || RpcError::Failed("That chat hasn't worked yet: there's no progress to read".into());
    let chat = rpc
        .workspace_host()
        .chat(chat_id)
        .ok()
        .flatten()
        .ok_or_else(|| RpcError::Failed(format!("No chat {chat_id}")))?;
    let orch = rpc.orchestration();
    if chat.device_id == rpc.doc_host_ref().device_id() {
        if orch.progress().current(orch, chat_id, Utc::now()).is_none() {
            return Err(nothing());
        }
        // The note runs on its own: a slow model never holds the caller past
        // ON_DEMAND_WAIT (the note lands on the row when it's done).
        let (task_orch, chat) = (orch.clone(), chat_id.to_owned());
        let task = tokio::spawn(async move { task_orch.progress().note(&task_orch, &chat, Why::OnDemand).await });
        return match tokio::time::timeout(ON_DEMAND_WAIT, task).await {
            Ok(Ok(Some(progress))) => Ok((progress, false)),
            Ok(_) => Err(nothing()),
            Err(_) => {
                let progress = orch.progress().current(orch, chat_id, Utc::now()).ok_or_else(nothing)?;
                Ok((progress, true))
            }
        };
    }
    // A host that predates progress writes no row and ignores the command:
    // worked out here instead.
    if proxy::needs_proxy(orch, &chat.device_id) {
        return proxy::check(rpc, chat_id).await.ok_or_else(nothing);
    }
    let row = orch
        .workspace()
        .read_orchestration_with(|doc| doc.progress(chat_id))
        .ok_or_else(nothing)?;
    let now = Utc::now();
    let stale = match row.note_at {
        None => true,
        Some(at) if now - at < NOTE_ON_DEMAND_EVERY => false,
        Some(at) => row.last_step_at.is_some_and(|step| step > at) || (row.state.active() && now - at >= NOTE_AUTO_EVERY),
    };
    if !stale {
        return Ok((row, false));
    }
    match rpc
        .doc_host_ref()
        .queue_command(chat_id, zeron_doc::SessionCommandPayload::ProgressCheck {})
    {
        Ok(_) => Ok((row, true)),
        Err(error) => {
            tracing::debug!(chat = %chat_id, %error, "progress: couldn't ask the host for a note");
            Ok((row, false))
        }
    }
}

/// `progressCheck`'s reply: the row, plus `refreshing` while another
/// device writes the fresh note.
pub(crate) async fn check_reply(rpc: &EngineRpc, chat_id: &str) -> Result<Value, RpcError> {
    let (progress, refreshing) = check(rpc, chat_id).await?;
    Ok(reply(&progress, refreshing))
}

/// The rows `WatchProgress` sends: chats working (or waiting to go on),
/// changed in the last 24 h, or in a crew; for an agent, only those it may
/// check.
pub(crate) fn visible(
    rows: &[Progress],
    crew: &HashSet<String>,
    allowed: Option<&dyn Fn(&str) -> bool>,
    now: DateTime<Utc>,
) -> Vec<Progress> {
    rows.iter()
        .filter(|p| p.state.active() || now - p.updated_at <= RECENT || crew.contains(&p.chat_id))
        .filter(|p| allowed.is_none_or(|ok| ok(&p.chat_id)))
        .cloned()
        .collect()
}

fn frame(rpc: &EngineRpc, viewer: Option<&Who>) -> Value {
    let orch = rpc.orchestration();
    let (mut rows, crew) = orch.workspace().read_orchestration_with(|doc| {
        let crew: HashSet<String> = doc.read_crew().into_iter().filter(|m| !m.released).map(|m| m.chat_id).collect();
        (doc.read_progress(), crew)
    });
    // Chats on older hosts: their progress as worked out here.
    for row in orch.progress().proxies().rows() {
        match rows.iter_mut().find(|r| r.chat_id == row.chat_id) {
            Some(synced) if synced.updated_at >= row.updated_at => {}
            Some(synced) => *synced = row,
            None => rows.push(row),
        }
    }
    let allowed: &dyn Fn(&str) -> bool = &|chat: &str| viewer.is_none_or(|who| may_check(rpc, who, chat));
    let rows = visible(&rows, &crew, Some(allowed), Utc::now());
    serde_json::to_value(rows).unwrap_or(Value::Array(Vec::new()))
}

/// `WatchProgress`: the rows now, then on every change (≤ 4/s). `viewer`
/// is an agent's caller; `None` is the person. While anyone watches, this
/// engine works out the progress of chats on older hosts (by proxy).
pub(crate) fn stream(rpc: EngineRpc, viewer: Option<Who>) -> futures::stream::BoxStream<'static, Value> {
    use futures::StreamExt;
    let orch = rpc.orchestration().clone();
    let watching = proxy::Watching::new(&orch);
    let mut rx = orch.workspace().watch_progress_rows();
    rx.mark_unchanged();
    let mut proxy_rx = orch.progress().proxies().subscribe();
    proxy_rx.mark_unchanged();
    // The first frame has them already.
    orch.progress().proxies().pass(&orch, &rpc, Utc::now());
    let first = frame(&rpc, viewer.as_ref());
    let state = (rx, proxy_rx, rpc, viewer, None::<Value>, watching);
    futures::stream::iter([first])
        .chain(futures::stream::unfold(state, |(mut rx, mut proxy_rx, rpc, viewer, mut last, watching)| async move {
            loop {
                tokio::select! {
                    changed = rx.changed() => changed.ok()?,
                    changed = proxy_rx.changed() => changed.ok()?,
                }
                tokio::time::sleep(super::PUBLISH_EVERY).await;
                rx.mark_unchanged();
                proxy_rx.mark_unchanged();
                let next = frame(&rpc, viewer.as_ref());
                if last.as_ref() != Some(&next) {
                    last = Some(next.clone());
                    return Some((next, (rx, proxy_rx, rpc, viewer, last, watching)));
                }
            }
        }))
        .boxed()
}

mod proxy;
#[cfg(test)]
mod tests;
