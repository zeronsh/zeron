//! What a chat's agent is doing right now (`Session::activity`) and how far
//! through its todo list it is (`Session::todo`), projected from the run's
//! events by its host. The session row is synced, so it's written only when
//! the projection changes, and a label-only change (the next file read) at
//! most every [`LABEL_EVERY`].

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use zeron_proto::orchestration::{ActivityKind, ChatActivity, TodoProgress, activity_of, todo_progress};
use zeron_proto::{AgentEvent, Session, SessionStatus, ToolCall};

use super::*;

/// The least time between two writes that only change the label.
const LABEL_EVERY: Duration = Duration::from_secs(2);

/// The label while no tool runs.
const THINKING: &str = "Thinking";

/// One run's projection. Fed every event that reaches the fold.
#[derive(Default)]
pub(crate) struct LiveTracker {
    /// Tool calls still running, oldest first: the newest is what the agent
    /// is doing.
    open: Vec<(String, ActivityKind, String)>,
    shown: Option<(ActivityKind, String)>,
    since: Option<DateTime<Utc>>,
    todo: Option<TodoProgress>,
    written_at: Option<Instant>,
    /// A label-only change not written yet.
    owed: bool,
}

/// What to write on the row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LiveUpdate {
    pub activity: Option<ChatActivity>,
    pub todo: Option<TodoProgress>,
}

impl LiveTracker {
    pub(crate) fn new(todo: Option<TodoProgress>) -> Self {
        Self { todo, ..Default::default() }
    }

    /// Take in one event; `Some` when the row should be written now.
    pub(crate) fn observe(&mut self, event: &AgentEvent, now: DateTime<Utc>, at: Instant) -> Option<LiveUpdate> {
        let todo_before = self.todo.clone();
        let mut working = true;
        match event {
            AgentEvent::ToolCall { call: ToolCall::Todo { items }, .. } => {
                self.todo = todo_progress(items);
            }
            AgentEvent::ToolCall { id, call } => {
                let (kind, label) = activity_of(call);
                match self.open.iter_mut().find(|(open, ..)| open == id) {
                    Some(slot) => {
                        slot.1 = kind;
                        slot.2 = label;
                    }
                    None => self.open.push((id.clone(), kind, label)),
                }
            }
            AgentEvent::ToolResult { id, .. } => self.open.retain(|(open, ..)| open != id),
            AgentEvent::Done { .. } => {
                self.open.clear();
                working = false;
            }
            _ => {}
        }
        let now_shown = working.then(|| match self.open.last() {
            Some((_, kind, label)) => (*kind, label.clone()),
            None => (ActivityKind::Thinking, THINKING.to_owned()),
        });
        let todo_changed = todo_before != self.todo;
        let kind_changed = now_shown.as_ref().map(|s| s.0) != self.shown.as_ref().map(|s| s.0);
        let label_changed = now_shown != self.shown;
        if label_changed {
            self.since = now_shown.as_ref().map(|_| now);
            self.shown = now_shown;
        }
        let label_due = self.written_at.is_none_or(|w| at.duration_since(w) >= LABEL_EVERY);
        if todo_changed || kind_changed || ((label_changed || self.owed) && label_due) {
            self.owed = false;
            self.written_at = Some(at);
            return Some(self.update());
        }
        if label_changed {
            self.owed = true;
        }
        None
    }

    fn update(&self) -> LiveUpdate {
        LiveUpdate {
            activity: self.shown.as_ref().map(|(kind, label)| ChatActivity {
                kind: *kind,
                label: label.clone(),
                since: self.since.unwrap_or_else(Utc::now),
            }),
            todo: self.todo.clone(),
        }
    }
}

/// The activity a row shows once its status is `status`: none unless the
/// agent works, thinking when a turn starts with nothing shown yet.
pub(crate) fn settle(row: &mut Session, status: SessionStatus, now: DateTime<Utc>) {
    match status {
        SessionStatus::Working => {
            if row.activity.is_none() {
                row.activity = Some(ChatActivity {
                    kind: ActivityKind::Thinking,
                    label: THINKING.to_owned(),
                    since: now,
                });
            }
        }
        SessionStatus::Idle | SessionStatus::AwaitingInput | SessionStatus::Errored => row.activity = None,
    }
}

impl Inner {
    /// Publish `chat_id`'s live activity and todo progress, mirrored like
    /// [`Self::set_background_tasks`]: only a change is written.
    pub(super) fn set_live(&self, chat_id: &str, live: LiveUpdate) {
        let session = {
            let mut statuses = lock(&self.statuses);
            let Some(entry) = statuses.get_mut(chat_id) else {
                return;
            };
            // Activity is only ever shown for a working agent.
            let activity = live.activity.filter(|_| entry.status == SessionStatus::Working);
            let same_activity = match (&entry.activity, &activity) {
                (Some(a), Some(b)) => a.kind == b.kind && a.label == b.label,
                (None, None) => true,
                _ => false,
            };
            if same_activity && entry.todo == live.todo {
                return;
            }
            entry.activity = activity;
            entry.todo = live.todo;
            let session = entry.clone();
            let mut list: Vec<Session> = statuses.values().cloned().collect();
            list.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
            self.sessions_tx.send_replace(list);
            session
        };
        if self.activity_tx.get().is_none()
            && let Some(ws) = self.workspace()
        {
            ws.record_session(&session);
        }
    }

    /// The todo progress `chat_id`'s row shows now (a new run starts from it).
    pub(super) fn live_todo(&self, chat_id: &str) -> Option<TodoProgress> {
        lock(&self.statuses).get(chat_id).and_then(|s| s.todo.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, call: ToolCall) -> AgentEvent {
        AgentEvent::ToolCall { id: id.into(), call }
    }

    fn result(id: &str) -> AgentEvent {
        AgentEvent::ToolResult { id: id.into(), is_error: false, output: None, diff: None, exit_code: None }
    }

    #[test]
    fn the_newest_running_tool_is_the_activity_and_thinking_fills_the_gaps() {
        let mut t = LiveTracker::new(None);
        let now = Utc::now();
        let start = Instant::now();
        let u = t
            .observe(&call("1", ToolCall::Exec { command: "cargo test -p x".into() }), now, start)
            .unwrap();
        assert_eq!(u.activity.as_ref().unwrap().kind, ActivityKind::Testing);
        // A refresh of the same call changes nothing.
        assert!(t.observe(&call("1", ToolCall::Exec { command: "cargo test -p x".into() }), now, start).is_none());
        let u = t
            .observe(&call("2", ToolCall::EditFile { path: "a/b.rs".into(), old_string: None, new_string: None }), now, start)
            .unwrap();
        assert_eq!(u.activity.unwrap().label, "Edit b.rs");
        // The edit ends: back to the test still running.
        let u = t.observe(&result("2"), now, start).unwrap();
        assert_eq!(u.activity.unwrap().kind, ActivityKind::Testing);
        let u = t.observe(&result("1"), now, start).unwrap();
        assert_eq!(u.activity.unwrap().kind, ActivityKind::Thinking);
        // Text streaming writes nothing.
        assert!(t.observe(&AgentEvent::TextDelta { text: "hi".into() }, now, start).is_none());
    }

    #[test]
    fn label_only_changes_are_throttled_and_owed() {
        let mut t = LiveTracker::new(None);
        let now = Utc::now();
        let start = Instant::now();
        assert!(t.observe(&call("1", ToolCall::ReadFile { path: "a.rs".into() }), now, start).is_some());
        // Another read right away: same kind, label owed, not written.
        assert!(t.observe(&call("2", ToolCall::ReadFile { path: "b.rs".into() }), now, start).is_none());
        // Later, any event pays what's owed.
        let later = start + LABEL_EVERY;
        let u = t.observe(&AgentEvent::TextDelta { text: "x".into() }, now, later).unwrap();
        assert_eq!(u.activity.unwrap().label, "Read b.rs");
    }

    #[test]
    fn todo_comes_from_the_newest_list_and_done_clears_activity() {
        use zeron_proto::{TodoItem, TodoStatus};
        let mut t = LiveTracker::new(None);
        let now = Utc::now();
        let start = Instant::now();
        let items = vec![TodoItem::new("a", TodoStatus::Completed), TodoItem::new("b", TodoStatus::InProgress)];
        let u = t.observe(&call(zeron_proto::LIVE_PLAN_TOOL_ID, ToolCall::Todo { items }), now, start).unwrap();
        assert_eq!(u.todo, Some(TodoProgress { done: 1, total: 2, current: Some("b".into()) }));
        let items = vec![TodoItem::new("a", TodoStatus::Completed), TodoItem::new("b", TodoStatus::Completed)];
        let u = t.observe(&call(zeron_proto::LIVE_PLAN_TOOL_ID, ToolCall::Todo { items }), now, start).unwrap();
        assert_eq!(u.todo.as_ref().unwrap().done, 2);
        let done = AgentEvent::Done {
            status: zeron_proto::DoneStatus::Completed,
            result: None,
            error: None,
            session_id: None,
        };
        let u = t.observe(&done, now, start).unwrap();
        assert_eq!(u.activity, None);
        assert_eq!(u.todo.unwrap().done, 2, "todo progress outlives the turn");
    }

    #[test]
    fn settle_clears_activity_unless_working() {
        let mut row = Session {
            last_completed_turn: None,
            chat_id: "c".into(),
            device_id: "d".into(),
            status: SessionStatus::Working,
            started_at: None,
            updated_at: Utc::now(),
            last_turn_outcome: None,
            terminal_activity: None,
            running_subagents: 0,
            background_tasks: Vec::new(),
            waiting: None,
            activity: None,
            todo: None,
        };
        settle(&mut row, SessionStatus::Working, Utc::now());
        assert_eq!(row.activity.as_ref().unwrap().kind, ActivityKind::Thinking);
        settle(&mut row, SessionStatus::Idle, Utc::now());
        assert_eq!(row.activity, None);
    }
}
