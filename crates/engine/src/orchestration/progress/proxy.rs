//! Progress by proxy: for a chat whose host predates progress (its device
//! row lacks `orchestration-progress-v1`, so it writes no row and ignores
//! `ProgressCheck`), the engine someone watches from works it out itself.
//!
//! - From what syncs: the host's session row (status, start, waiting) and
//!   the chat's transcript (the newest todo list for steps and the current
//!   item, the newest tool call for what's running). A change seen here is a
//!   step at the time it's seen, so the times are coarser than a host's.
//! - Only while someone watches (`WatchProgress`), and only for chats that
//!   are working (or waiting to go on), or finished in the last 24 h (from
//!   their session row alone: no transcript is read for those). A working
//!   chat's transcript is read at most every [`READ_EVERY`].
//! - Never written to the registry: several devices may watch, and none of
//!   them is the host. The rows ride `WatchProgress` with `proxy: true`.
//! - `progressCheck` makes the note here, from the synced transcript, with
//!   the host's instructions, limits and redaction. No automatic notes.

use tokio::sync::watch;
use zeron_proto::orchestration::todo_progress;

use super::*;

/// The least time between two reads of a working chat's transcript.
pub(crate) const READ_EVERY: TimeDelta = TimeDelta::seconds(10);

/// What the transcript says, carried between reads.
#[derive(Debug, Clone, Default)]
struct Derived {
    todo: Option<TodoProgress>,
    activity: Option<ChatActivity>,
    /// The running tool part's id (its start is when it was first seen).
    tool_part: Option<String>,
}

#[derive(Debug, Clone)]
struct ProxyEntry {
    tracker: Tracker,
    shown: Option<Progress>,
    clock: NoteClock,
    derived: Derived,
    /// The transcript last read (fingerprint) and when.
    read: Option<(String, DateTime<Utc>)>,
}

/// The chats this engine follows by proxy.
pub(crate) struct Proxies {
    chats: Mutex<HashMap<String, ProxyEntry>>,
    watchers: std::sync::atomic::AtomicUsize,
    version: watch::Sender<u64>,
    wake: tokio::sync::Notify,
}

impl Default for Proxies {
    fn default() -> Self {
        Self {
            chats: Mutex::new(HashMap::new()),
            watchers: std::sync::atomic::AtomicUsize::new(0),
            version: watch::Sender::new(0),
            wake: tokio::sync::Notify::new(),
        }
    }
}

/// Held by each `WatchProgress` subscriber: proxies are worked out only
/// while one is.
pub(crate) struct Watching(Orchestration);

impl Watching {
    pub(crate) fn new(orch: &Orchestration) -> Self {
        let proxies = orch.progress().proxies();
        proxies.watchers.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        proxies.wake.notify_one();
        Self(orch.clone())
    }
}

impl Drop for Watching {
    fn drop(&mut self) {
        self.0.progress().proxies().watchers.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// Whether a chat hosted on `device_id` needs its progress worked out here:
/// its host isn't this engine and doesn't say it writes progress.
pub(crate) fn needs_proxy(orch: &Orchestration, device_id: &str) -> bool {
    device_id != orch.device_id()
        && !orch.workspace().read_devices().unwrap_or_default().iter().any(|d| {
            d.id == device_id && d.supports(zeron_proto::capabilities::ORCHESTRATION_PROGRESS_V1)
        })
}

fn ms(at: i64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(at)
}

/// What `entries` say now: the newest todo list, and the newest tool call
/// of the last reply when it is still running (else thinking).
fn derive(entries: &[SessionMessageEntry], before: &Derived, now: DateTime<Utc>) -> Derived {
    let todo = entries.iter().rev().flat_map(|e| e.parts.iter().rev()).find_map(|p| match p {
        MessagePart::Tool { call: ToolCall::Todo { items }, .. } => Some(todo_progress(items)),
        _ => None,
    });
    let running = entries
        .iter()
        .rev()
        .find(|e| e.role == MessageRole::Assistant)
        .and_then(|e| {
            e.parts.iter().rev().find_map(|p| match p {
                MessagePart::Tool { id, call, resolved: false, .. } if !matches!(call, ToolCall::Todo { .. }) => {
                    Some((id.clone(), call))
                }
                _ => None,
            })
        });
    let (activity, tool_part) = match running {
        Some((id, call)) => {
            let (kind, label) = activity_of(call);
            let since = match (&before.tool_part, &before.activity) {
                (Some(seen), Some(a)) if *seen == id => a.since,
                _ => now,
            };
            (Some(ChatActivity { kind, label, since }), Some(id))
        }
        None => {
            let since = before.activity.as_ref().filter(|a| a.kind == ActivityKind::Thinking).map_or(now, |a| a.since);
            (Some(ChatActivity { kind: ActivityKind::Thinking, label: "Thinking".into(), since }), None)
        }
    };
    Derived { todo: todo.flatten(), activity, tool_part }
}

fn active(session: &Session) -> bool {
    matches!(session.status, SessionStatus::Working | SessionStatus::AwaitingInput) || session.waiting.is_some()
}

impl Proxies {
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.version.subscribe()
    }

    fn watched(&self) -> bool {
        self.watchers.load(std::sync::atomic::Ordering::Acquire) > 0
    }

    /// The rows as worked out here.
    pub(crate) fn rows(&self) -> Vec<Progress> {
        lock(&self.chats).values().filter_map(|e| e.shown.clone()).collect()
    }

    /// Every chat on an older host, from its session row (and, while it
    /// works, its transcript), when it is worth following. Nothing while no
    /// one watches.
    pub(crate) fn pass(&self, orch: &Orchestration, rpc: &EngineRpc, now: DateTime<Utc>) {
        if !self.watched() {
            return;
        }
        let sessions = orch.workspace().read_sessions().unwrap_or_default();
        let mut hosts: HashMap<String, bool> = HashMap::new();
        for session in sessions {
            if untracked(&session.chat_id) {
                continue;
            }
            let proxied = *hosts
                .entry(session.device_id.clone())
                .or_insert_with(|| needs_proxy(orch, &session.device_id));
            if proxied {
                self.observe(rpc, &session, now, false);
            }
        }
    }

    /// Take in `session` (a chat on an older host). `force` reads its
    /// transcript whatever the last read.
    fn observe(&self, rpc: &EngineRpc, session: &Session, now: DateTime<Utc>, force: bool) {
        let chat = session.chat_id.as_str();
        let (known, last_read) = {
            let chats = lock(&self.chats);
            let entry = chats.get(chat);
            (entry.is_some(), entry.and_then(|e| e.read.clone()))
        };
        let live = active(session);
        let recent = session.last_turn_outcome.is_some_and(|o| now - o.finished_at <= RECENT);
        if !known && !live && !recent && !force {
            return;
        }
        // Only a chat at work (or asked about) has its transcript read.
        let due = force || (live && last_read.as_ref().is_none_or(|(_, at)| now - *at >= READ_EVERY));
        let entries = due
            .then(|| rpc.doc_host_ref().open(chat).ok().and_then(|h| h.doc().read_entries().ok()))
            .flatten();
        let mut chats = lock(&self.chats);
        let entry = chats.entry(chat.to_owned()).or_insert_with(|| ProxyEntry {
            tracker: Tracker::new(chat, &session.device_id),
            shown: None,
            clock: NoteClock::default(),
            derived: Derived::default(),
            read: None,
        });
        let mut first_read = false;
        if let Some(entries) = &entries {
            let fp = fingerprint(entries);
            if entry.read.as_ref().is_none_or(|(seen, _)| *seen != fp) {
                first_read = entry.read.is_none();
                entry.derived = derive(entries, &entry.derived, now);
            }
            entry.read = Some((fp, now));
        }
        let mut seen = session.clone();
        seen.todo = session.todo.clone().or_else(|| entry.derived.todo.clone());
        seen.activity = session.activity.clone().or_else(|| entry.derived.activity.clone());
        entry.tracker.observe(&seen, now);
        if first_read
            && let Some(at) = entries.as_ref().and_then(|e| e.last()).and_then(|e| ms(e.created_at))
        {
            entry.tracker.seed_step(at.min(now));
        }
        let mut next = entry.tracker.snapshot(now);
        next.proxy = true;
        next.refreshing = entry.clock.in_flight;
        if needs_write(entry.shown.as_ref(), &next) {
            entry.shown = Some(next);
            drop(chats);
            self.version.send_modify(|v| *v = v.wrapping_add(1));
        }
    }

    fn shown(&self, chat: &str) -> Option<Progress> {
        lock(&self.chats).get(chat).and_then(|e| e.shown.clone())
    }
}

/// Follow chats on older hosts while anyone watches: on every session row
/// change, every [`TICK`] (working ones' transcripts), and when a watcher
/// comes. Nothing at all while no one watches.
pub(crate) fn start(orch: Orchestration) {
    let run = orch.clone();
    run.spawn(async move {
        let mut rows = orch.workspace().watch_session_rows();
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let proxies = orch.progress().proxies();
            tokio::select! {
                changed = rows.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    rows.mark_unchanged();
                }
                _ = tick.tick() => {}
                _ = proxies.wake.notified() => {}
            }
            let Some(rpc) = orch.rpc() else { continue };
            proxies.pass(&orch, &rpc, Utc::now());
        }
    });
}

/// `progressCheck` on a chat on an older host: worked out here now, its
/// note made here from the synced transcript (at most every 15 s, the
/// latest otherwise; at most [`ON_DEMAND_WAIT`] waited for). `None` when
/// the chat has no session row to go on.
pub(crate) async fn check(rpc: &EngineRpc, chat_id: &str) -> Option<(Progress, bool)> {
    let orch = rpc.orchestration().clone();
    let session = orch
        .workspace()
        .read_sessions()
        .unwrap_or_default()
        .into_iter()
        .find(|s| s.chat_id == chat_id)?;
    let proxies = orch.progress().proxies();
    let now = Utc::now();
    proxies.observe(rpc, &session, now, true);
    let claimed = {
        let mut chats = lock(&proxies.chats);
        chats.get_mut(chat_id).map(|e| e.clock.claim(Why::OnDemand, true, now))?
    };
    if claimed == Claim::Latest {
        return proxies.shown(chat_id).map(|p| (p, false));
    }
    let (task_orch, task_rpc, chat) = (orch.clone(), rpc.clone(), chat_id.to_owned());
    let task = tokio::spawn(async move { note(&task_orch, &task_rpc, &chat).await });
    match tokio::time::timeout(ON_DEMAND_WAIT, task).await {
        Ok(_) => proxies.shown(chat_id).map(|p| (p, false)),
        Err(_) => proxies.shown(chat_id).map(|p| (p, true)),
    }
}

/// The note, made here (the claim is held): the clock released after.
async fn note(orch: &Orchestration, rpc: &EngineRpc, chat_id: &str) {
    let proxies = orch.progress().proxies();
    let entries = rpc.doc_host_ref().open(chat_id).ok().and_then(|h| h.doc().read_entries().ok());
    let now = Utc::now();
    let mut made = None;
    if let Some(entries) = entries {
        let fp = fingerprint(&entries);
        let (progress, started, last_fp) = {
            let chats = lock(&proxies.chats);
            let Some(entry) = chats.get(chat_id) else { return };
            (entry.tracker.snapshot(now), entry.tracker.started_at(), entry.clock.fingerprint.clone())
        };
        if last_fp.as_deref() != Some(fp.as_str()) {
            let note = orch.progress().note_from(orch, rpc, chat_id, &entries, &progress, started, now).await;
            made = note.map(|n| (n, fp));
        }
    }
    let mut chats = lock(&proxies.chats);
    let Some(entry) = chats.get_mut(chat_id) else { return };
    entry.clock.done(made.as_ref().map(|(_, fp)| fp.clone()));
    if let Some((note, _)) = made {
        entry.tracker.set_note(note, Utc::now());
    }
    let mut next = entry.tracker.snapshot(Utc::now());
    next.proxy = true;
    entry.shown = Some(next);
    drop(chats);
    proxies.version.send_modify(|v| *v = v.wrapping_add(1));
}
