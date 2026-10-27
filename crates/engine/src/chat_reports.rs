//! Chats report to the chat that started them by themselves, never by
//! messaging it (`zeron_proto::chat_events`):
//!
//! - a **side chat**, or an **agent** of a task, when a turn its parent gave
//!   it finishes, fails, or stops on a question;
//! - a task's **coordinator** through its `report` tool (see the RPC); this
//!   module only backs that up: a coordinator that ends a turn with nothing
//!   left running and no report reports its last reply as the task's result,
//!   so a task never ends silently.
//!
//! The engine hosting the parent reports, reading each child's turn off its
//! own replica of the child's chat: a child on another machine can't write
//! to its parent's chat, and its parent's machine is woken when it stops
//! working (the edge does that). Transitions are the desktop banners' own
//! ([`SessionNotificationState`]): once per completed turn, interrupts never.
//! A turn the user started by typing into a side chat stays between them:
//! only a turn whose prompt came from the parent is reported.
//!
//! At boot, a child whose last turn ended recently and was never reported
//! (the parent's machine was asleep, or the engine down) is reported then;
//! the parent's doc remembers the last turn reported per child, so nothing
//! is reported twice.

use std::collections::HashMap;

use tokio::sync::watch;

use zeron_doc::{ChatNotice, notice_id};
use zeron_proto::chat_events::EventKind;
use zeron_proto::{Chat, Session, SessionStatus};

use crate::desktop_notify::{Ping, SessionNotificationState};
use crate::doc_host::DocHost;

/// How far back a boot catches up on turns that ended unreported.
const CATCH_UP_WINDOW_MS: i64 = 6 * 60 * 60 * 1000;

/// How long a report waits for its transcript to catch up with the status
/// that prompted it, and how often it looks meanwhile. A child on another
/// machine syncs its transcript after its status.
const CATCH_UP: std::time::Duration = std::time::Duration::from_secs(30);
const CATCH_UP_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// Watch every session (`sessions`: this engine's runs merged with the
/// registry's rows for chats hosted elsewhere) and report the children among
/// `chats` of the chats this engine hosts. Runs as one of `doc_host`'s
/// workers.
pub(crate) async fn run(
    doc_host: DocHost,
    mut sessions: watch::Receiver<Vec<Session>>,
    mut chats: watch::Receiver<Vec<Chat>>,
) {
    let mut reporter = Reporter::default();
    loop {
        let due = {
            let sessions = sessions.borrow_and_update();
            let chats = chats.borrow_and_update();
            reporter.observe(&sessions, &chats, |parent| doc_host.hosts(parent))
        };
        for (chat, ping) in due {
            report_or_retry(&doc_host, chat, ping);
        }
        tokio::select! {
            changed = sessions.changed() => if changed.is_err() { break },
            changed = chats.changed() => if changed.is_err() { break },
        }
    }
}

/// Per-session baselines, like the desktop notifier's.
#[derive(Default)]
struct Reporter {
    baselines: HashMap<String, SessionNotificationState>,
}

impl Reporter {
    /// The children of hosted chats whose session just finished, failed or
    /// stopped on a question. A child first seen settled (at boot, or its
    /// first frame) is due once more: [`report`] skips a turn already
    /// reported, and one that ended long ago.
    fn observe(
        &mut self,
        sessions: &[Session],
        chats: &[Chat],
        hosts: impl Fn(&str) -> bool,
    ) -> Vec<(Chat, Ping)> {
        let now = chrono::Utc::now();
        let mut due = Vec::new();
        for session in sessions {
            let status = SessionNotificationState::new(session, now);
            let prev = self
                .baselines
                .insert(session.chat_id.clone(), status.clone());
            let Some(chat) = chats.iter().find(|chat| chat.id == session.chat_id) else {
                continue;
            };
            let Some(parent) = chat.parent_chat_id.as_deref() else {
                continue;
            };
            if zeron_proto::voice::is_orchestrator_chat(parent) || !hosts(parent) {
                continue;
            }
            let ping = match prev {
                Some(prev) => status.ping_since(&prev, false),
                None => first_sight(session, now),
            };
            if let Some(ping) = ping {
                due.push((chat.clone(), ping));
            }
        }
        due
    }
}

/// What a child's session first seen settled may still owe its parent.
fn first_sight(session: &Session, now: chrono::DateTime<chrono::Utc>) -> Option<Ping> {
    let recent = |at: chrono::DateTime<chrono::Utc>| {
        now.signed_duration_since(at).num_milliseconds() <= CATCH_UP_WINDOW_MS
    };
    match session.status {
        SessionStatus::AwaitingInput => Some(Ping::Request),
        SessionStatus::Errored if recent(session.updated_at) => Some(Ping::Attention),
        SessionStatus::Idle
            if session.last_completed_turn.is_some()
                && session
                    .last_turn_outcome
                    .is_none_or(|o| o.status == zeron_proto::DoneStatus::Completed && recent(o.finished_at)) =>
        {
            Some(Ping::Done)
        }
        _ => None,
    }
}

/// What [`report`] made of a transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Report {
    /// Posted, already there, or nothing to report.
    Settled,
    /// The status moved before the transcript caught up (a question's part,
    /// a final reply still syncing): look again shortly.
    NotYet,
}

fn report_or_retry(doc_host: &DocHost, chat: Chat, ping: Ping) {
    if report(doc_host, &chat, ping, false) == Report::Settled {
        return;
    }
    let host = doc_host.clone();
    doc_host.spawn_report(async move {
        let deadline = tokio::time::Instant::now() + CATCH_UP;
        loop {
            tokio::time::sleep(CATCH_UP_POLL).await;
            let last = tokio::time::Instant::now() >= deadline;
            if report(&host, &chat, ping, last) == Report::Settled || last {
                return;
            }
        }
    });
}

/// Leave `chat`'s event on its parent's doc. `last` reports whatever the
/// transcript has rather than waiting for more.
fn report(doc_host: &DocHost, chat: &Chat, ping: Ping, last: bool) -> Report {
    let Some(parent) = chat.parent_chat_id.as_deref() else {
        return Report::Settled;
    };
    let Some(turn) = doc_host.ended_turn(&chat.id) else {
        return if last { Report::Settled } else { Report::NotYet };
    };
    if doc_host.last_reported(parent, &chat.id).as_deref() == Some(turn.turn_key.as_str()) {
        return Report::Settled;
    }
    if turn.streaming && ping == Ping::Done && !last {
        return Report::NotYet;
    }
    let event = if chat.is_coordinator() {
        // Its own `report` is how a coordinator speaks; this only covers a
        // task that ends without one.
        match ping {
            Ping::Done if doc_host.reported_since(parent, &chat.id, turn.started_at) => None,
            Ping::Done if doc_host.agents_at_work(&chat.id) => None,
            Ping::Done if turn.reply.is_empty() && !last => return Report::NotYet,
            Ping::Done => Some((EventKind::TaskDone, turn.turn_key.clone(), turn.reply.clone())),
            Ping::Attention => Some((
                EventKind::TaskBlocked,
                turn.turn_key.clone(),
                turn.error.clone().unwrap_or_else(|| turn.reply.clone()),
            )),
            Ping::Request => match turn.pending_input.clone() {
                Some((request_id, question)) => Some((EventKind::TaskQuestion, request_id, question)),
                None => return if last { Report::Settled } else { Report::NotYet },
            },
        }
    } else {
        if turn.delegator.as_deref() != Some(parent) {
            tracing::debug!(chat = %chat.id, "turn was not delegated; not reporting");
            return Report::Settled;
        }
        match ping {
            Ping::Done if turn.reply.is_empty() && !last => return Report::NotYet,
            Ping::Done => Some((EventKind::Finished, turn.turn_key.clone(), turn.reply.clone())),
            Ping::Attention => Some((
                EventKind::Failed,
                turn.turn_key.clone(),
                turn.error.clone().unwrap_or_else(|| turn.reply.clone()),
            )),
            Ping::Request => match turn.pending_input.clone() {
                Some((request_id, question)) => Some((EventKind::NeedsInput, request_id, question)),
                None => return if last { Report::Settled } else { Report::NotYet },
            },
        }
    };
    let Some((kind, key, text)) = event else {
        return Report::Settled;
    };
    let notice = ChatNotice {
        id: notice_id(kind, &chat.id, &key),
        kind,
        chat_id: chat.id.clone(),
        title: chat.title.clone().unwrap_or_else(|| "Untitled chat".into()),
        text,
        issued_by: doc_host.device_id().to_owned(),
        issued_at: crate::now_ms(),
        delivered_at: None,
    };
    // A question is per request, not per turn: the turn may ask again.
    let turn_key = (kind != EventKind::NeedsInput && kind != EventKind::TaskQuestion)
        .then_some(turn.turn_key.as_str());
    match doc_host.post_report(parent, &notice, turn_key) {
        Ok(true) => tracing::info!(chat = %chat.id, parent, ?kind, "chat reported to its parent"),
        Ok(false) => {}
        Err(err) => tracing::warn!(chat = %chat.id, parent, error = %err, "chat report failed"),
    }
    Report::Settled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(chat_id: &str, status: SessionStatus, turn: Option<&str>) -> Session {
        Session {
            last_turn_outcome: None,
            terminal_activity: None,
            last_completed_turn: turn.map(str::to_owned),
            chat_id: chat_id.into(),
            device_id: "dev".into(),
            status,
            started_at: None,
            updated_at: chrono::Utc::now(),
            running_subagents: 0,
        }
    }

    fn chat(id: &str, parent: Option<&str>) -> Chat {
        let mut chat: Chat = serde_json::from_value(serde_json::json!({
            "id": id,
            "deviceId": "dev",
            "archived": false,
            "createdAt": chrono::Utc::now(),
        }))
        .unwrap();
        chat.parent_chat_id = parent.map(str::to_owned);
        chat
    }

    #[test]
    fn only_children_of_hosted_chats_report_on_a_transition() {
        use SessionStatus::*;
        let chats = [
            chat("side", Some("main")),
            chat("main", None),
            chat("elsewhere", Some("theirs")),
        ];
        let hosts = |parent: &str| parent == "main";
        let mut reporter = Reporter::default();
        assert!(reporter
            .observe(
                &[session("side", Working, None), session("main", Working, None)],
                &chats,
                hosts,
            )
            .is_empty());
        let due = reporter.observe(
            &[
                session("side", Idle, Some("t1")),
                session("main", Idle, Some("t1")),
                session("elsewhere", Idle, Some("t1")),
            ],
            &chats,
            hosts,
        );
        assert_eq!(due.len(), 1);
        assert_eq!((due[0].0.id.as_str(), due[0].1), ("side", Ping::Done));
        // The same completion seen again is not a new one.
        assert!(reporter
            .observe(&[session("side", Idle, Some("t1"))], &chats, hosts)
            .is_empty());
        let due = reporter.observe(&[session("side", AwaitingInput, Some("t1"))], &chats, hosts);
        assert_eq!(due[0].1, Ping::Request);
    }

    #[test]
    fn a_child_first_seen_settled_is_owed_a_report_unless_it_is_old() {
        use SessionStatus::*;
        let chats = [chat("side", Some("main"))];
        let mut reporter = Reporter::default();
        let due = reporter.observe(&[session("side", Idle, Some("t1"))], &chats, |_| true);
        assert_eq!(due[0].1, Ping::Done);

        let mut old = session("side", Idle, Some("t1"));
        old.last_turn_outcome = Some(zeron_proto::NativeTurnOutcome {
            status: zeron_proto::DoneStatus::Completed,
            finished_at: chrono::Utc::now() - chrono::Duration::days(2),
        });
        let mut reporter = Reporter::default();
        assert!(reporter.observe(&[old], &chats, |_| true).is_empty());
        // Never ran: nothing to report.
        let mut reporter = Reporter::default();
        assert!(reporter.observe(&[session("side", Idle, None)], &chats, |_| true).is_empty());
    }
}
