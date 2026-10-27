//! A side chat reports to the chat that started it by itself: when a turn
//! its parent delegated finishes, fails, or stops on a question, this device
//! (the side chat's host) leaves a notice on the parent's doc, and the
//! parent's host hands it to the parent's agent (`DocHost::drain_notices`).
//!
//! Only runs this engine executes are watched, so exactly one device reports
//! each turn, and the transitions are the desktop banners' own
//! ([`SessionNotificationState`]): seeded silently at boot, a run finished
//! once per completed turn, interrupts never. Turns the user started by
//! typing into the side chat stay between them: only a turn whose prompt came
//! from the parent is reported.

use std::collections::HashMap;

use tokio::sync::watch;

use zeron_doc::{ChatNotice, notice_id};
use zeron_proto::side_chat_updates::UpdateKind;
use zeron_proto::{Chat, Session};

use crate::desktop_notify::{Ping, SessionNotificationState};
use crate::doc_host::DocHost;

/// Watch this engine's runs (`sessions`) and report the side chats among
/// `chats` to their parents. Runs as one of `doc_host`'s workers.
pub(crate) async fn run(
    doc_host: DocHost,
    mut sessions: watch::Receiver<Vec<Session>>,
    mut chats: watch::Receiver<Vec<Chat>>,
) {
    let device_id = doc_host.device_id().to_owned();
    let mut reporter = Reporter::default();
    reporter.observe(&sessions.borrow_and_update(), &chats.borrow_and_update());
    loop {
        tokio::select! {
            changed = sessions.changed() => {
                if changed.is_err() { break; }
            }
            changed = chats.changed() => {
                if changed.is_err() { break; }
            }
        }
        let due = reporter.observe(&sessions.borrow_and_update(), &chats.borrow_and_update());
        for (chat, ping) in due {
            if report(&doc_host, &device_id, &chat, ping, false) == Report::NotYet {
                retry(doc_host.clone(), device_id.clone(), chat, ping);
            }
        }
    }
}

/// Per-session baselines, like the desktop notifier's.
#[derive(Default)]
struct Reporter {
    baselines: HashMap<String, SessionNotificationState>,
    seeded: bool,
}

impl Reporter {
    /// The side chats whose session just finished, failed or stopped on a
    /// question. The sessions of the first sight (boot) seed silently; one
    /// that appears later is new work, whose first sight may already be its
    /// finished turn.
    fn observe(&mut self, sessions: &[Session], chats: &[Chat]) -> Vec<(Chat, Ping)> {
        let now = chrono::Utc::now();
        let seeded = std::mem::replace(&mut self.seeded, true);
        let mut due = Vec::new();
        for session in sessions {
            let status = SessionNotificationState::new(session, now);
            let prev = self
                .baselines
                .insert(session.chat_id.clone(), status.clone())
                .or_else(|| seeded.then(|| SessionNotificationState::new(&blank(session), now)));
            let Some(chat) = chats.iter().find(|chat| chat.id == session.chat_id) else {
                continue;
            };
            let Some(parent) = chat.parent_chat_id.as_deref() else {
                continue;
            };
            if zeron_proto::voice::is_orchestrator_chat(parent) {
                continue;
            }
            if let Some(prev) = prev
                && let Some(ping) = status.ping_since(&prev, false)
            {
                due.push((chat.clone(), ping));
            }
        }
        due
    }
}

/// A session as it was before its first turn.
fn blank(session: &Session) -> Session {
    Session {
        last_turn_outcome: None,
        terminal_activity: None,
        last_completed_turn: None,
        status: zeron_proto::SessionStatus::Idle,
        running_subagents: 0,
        ..session.clone()
    }
}

/// What [`report`] made of a transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Report {
    /// Posted, already there, or nothing to report.
    Settled,
    /// The status moved before the transcript caught up (a question's part,
    /// a final reply still landing): look again shortly.
    NotYet,
}

/// How long a report waits for its transcript to catch up, and how often it
/// looks meanwhile.
const CATCH_UP: std::time::Duration = std::time::Duration::from_secs(5);
const CATCH_UP_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// Look again for `chat`'s update until its transcript has it, then report
/// what there is.
fn retry(doc_host: DocHost, device_id: String, chat: Chat, ping: Ping) {
    let host = doc_host.clone();
    host.spawn_report(async move {
        let deadline = tokio::time::Instant::now() + CATCH_UP;
        loop {
            tokio::time::sleep(CATCH_UP_POLL).await;
            let last = tokio::time::Instant::now() >= deadline;
            if report(&doc_host, &device_id, &chat, ping, last) == Report::Settled || last {
                return;
            }
        }
    });
}

/// Leave `chat`'s update on its parent's doc, when its parent delegated the
/// turn. `last` reports whatever the transcript has rather than waiting.
fn report(doc_host: &DocHost, device_id: &str, chat: &Chat, ping: Ping, last: bool) -> Report {
    let Some(parent) = chat.parent_chat_id.as_deref() else {
        return Report::Settled;
    };
    let Some(turn) = doc_host.ended_turn(&chat.id) else {
        return Report::Settled;
    };
    if turn.delegator.as_deref() != Some(parent) {
        tracing::debug!(chat = %chat.id, "side chat turn was not delegated; not reporting");
        return Report::Settled;
    }
    let (kind, key, text) = match ping {
        Ping::Done if turn.reply.is_empty() && !last => return Report::NotYet,
        Ping::Done => (UpdateKind::Finished, turn.turn_key, turn.reply),
        Ping::Attention => (
            UpdateKind::Failed,
            turn.turn_key,
            turn.error.unwrap_or(turn.reply),
        ),
        Ping::Request => {
            let Some((request_id, question)) = turn.pending_input else {
                return if last {
                    Report::Settled
                } else {
                    Report::NotYet
                };
            };
            (UpdateKind::NeedsInput, request_id, question)
        }
    };
    let notice = ChatNotice {
        id: notice_id(kind, &chat.id, &key),
        kind,
        chat_id: chat.id.clone(),
        title: chat
            .title
            .clone()
            .unwrap_or_else(|| "Untitled side chat".into()),
        text,
        issued_by: device_id.to_owned(),
        issued_at: crate::now_ms(),
        delivered_at: None,
    };
    match doc_host.post_notice(parent, &notice) {
        Ok(true) => {
            tracing::info!(chat = %chat.id, parent, ?kind, "side chat reported to its parent")
        }
        Ok(false) => {}
        Err(err) => {
            tracing::warn!(chat = %chat.id, parent, error = %err, "side chat report failed")
        }
    }
    Report::Settled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(chat_id: &str, status: zeron_proto::SessionStatus, turn: Option<&str>) -> Session {
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
    fn only_side_chats_report_and_only_on_a_transition() {
        use zeron_proto::SessionStatus::*;
        let chats = [chat("side", Some("main")), chat("main", None)];
        let mut reporter = Reporter::default();
        assert!(
            reporter
                .observe(
                    &[
                        session("side", Working, None),
                        session("main", Working, None)
                    ],
                    &chats
                )
                .is_empty()
        );
        let due = reporter.observe(
            &[
                session("side", Idle, Some("t1")),
                session("main", Idle, Some("t1")),
            ],
            &chats,
        );
        assert_eq!(due.len(), 1);
        assert_eq!((due[0].0.id.as_str(), due[0].1), ("side", Ping::Done));
        // The same completion seen again is not a new one.
        assert!(
            reporter
                .observe(&[session("side", Idle, Some("t1"))], &chats)
                .is_empty()
        );
        let due = reporter.observe(&[session("side", AwaitingInput, Some("t1"))], &chats);
        assert_eq!(due[0].1, Ping::Request);
    }

    #[test]
    fn a_side_chat_first_seen_finished_after_boot_still_reports() {
        use zeron_proto::SessionStatus::*;
        let chats = [chat("side", Some("main")), chat("old", Some("main"))];
        let mut reporter = Reporter::default();
        // Boot: what already happened stays quiet.
        assert!(
            reporter
                .observe(&[session("old", Idle, Some("t0"))], &chats)
                .is_empty()
        );
        let due = reporter.observe(
            &[
                session("old", Idle, Some("t0")),
                session("side", Idle, Some("t1")),
            ],
            &chats,
        );
        assert_eq!(due.len(), 1);
        assert_eq!((due[0].0.id.as_str(), due[0].1), ("side", Ping::Done));
    }
}
