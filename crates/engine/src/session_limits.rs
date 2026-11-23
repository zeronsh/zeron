//! A chat its provider's usage limit stopped, on its session row
//! (`Session::limited`, [`zeron_proto::usage_limit`]): recorded as the turn
//! ends on the limit, in the same row write as its `Errored`, and cleared
//! when its next turn starts. What happens meanwhile (the account it ran on,
//! the reset, the resume, an account switch) is
//! [`crate::usage_limits`]'.

use super::*;
use crate::run_journal::LimitMark;
use crate::usage_limit_text::LimitHit;
use zeron_proto::usage_limit::SessionLimit;

impl Inner {
    /// The run ended `Errored` because the usage limit stopped its turn:
    /// settle the row with the limit. A limit right after a resume of an
    /// earlier one (that limit was cleared as the resumed turn started)
    /// counts on from it (`attempt`).
    pub(super) fn set_status_limited(
        &self,
        chat_id: &str,
        harness: HarnessId,
        completed_turn: Option<String>,
        outcome: Option<DoneStatus>,
        hit: LimitHit,
    ) {
        let now = Utc::now();
        let previous = lock(&self.limit_resumed).remove(chat_id);
        let limit = SessionLimit {
            provider: harness.as_str().to_owned(),
            account_slot: previous.as_ref().and_then(|p| p.account_slot.clone()),
            account_label: previous.as_ref().and_then(|p| p.account_label.clone()),
            window: hit.window,
            until: hit.until,
            since: now,
            attempt: previous.as_ref().map_or(0, |p| p.attempt.saturating_add(1)),
            switched_to: None,
        };
        tracing::info!(chat = %chat_id, until = ?limit.until, window = ?limit.window,
            attempt = limit.attempt, "usage limit stopped the turn");
        self.journal.note_limited(
            chat_id,
            &LimitMark { chat_id: chat_id.to_owned(), limit: limit.clone(), resumed_at: None },
        );
        self.update_status(chat_id, SessionStatus::Errored, false, completed_turn, outcome, Some(limit));
    }

    /// Change `chat_id`'s limit in place, publishing and storing the row like
    /// any transition when it changed (and its mark beside the journal:
    /// `resumed_at` with it). The limit after; `None` without a row.
    pub(super) fn update_limited(
        &self,
        chat_id: &str,
        change: impl FnOnce(&mut Option<SessionLimit>),
        resumed_at: Option<chrono::DateTime<Utc>>,
    ) -> Option<Option<SessionLimit>> {
        let (session, after) = {
            let mut statuses = lock(&self.statuses);
            let entry = statuses.get_mut(chat_id)?;
            let before = entry.limited.clone();
            change(&mut entry.limited);
            let after = entry.limited.clone();
            match &after {
                Some(limit) => self.journal.note_limited(
                    chat_id,
                    &LimitMark { chat_id: chat_id.to_owned(), limit: limit.clone(), resumed_at },
                ),
                None => self.journal.clear_limited(chat_id),
            }
            if after == before {
                return Some(after);
            }
            entry.updated_at = Utc::now();
            let session = entry.clone();
            let mut list: Vec<Session> = statuses.values().cloned().collect();
            list.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
            self.sessions_tx.send_replace(list);
            (session, after)
        };
        if self.activity_tx.get().is_none()
            && let Some(ws) = self.workspace()
        {
            ws.record_session(&session);
        }
        Some(after)
    }
}

impl SessionsEngine {
    /// Every row here holding a limit.
    pub(crate) fn limited_rows(&self) -> Vec<Session> {
        lock(&self.inner.statuses).values().filter(|row| row.limited.is_some()).cloned().collect()
    }

    /// Change `chat_id`'s limit in place (see [`Inner::update_limited`]).
    pub(crate) fn update_limited(
        &self,
        chat_id: &str,
        change: impl FnOnce(&mut Option<SessionLimit>),
        resumed_at: Option<chrono::DateTime<Utc>>,
    ) -> Option<Option<SessionLimit>> {
        self.inner.update_limited(chat_id, change, resumed_at)
    }

    /// Bring back a limit marked before the engine restarted: on the chat's
    /// row when it has one with no limit of its own (one that ran since
    /// moved on: its mark goes), else a row as the limit left it (its turn
    /// failed on it).
    pub(crate) fn restore_limited(&self, mark: &LimitMark) -> bool {
        let chat_id = mark.chat_id.as_str();
        let session = {
            let mut statuses = lock(&self.inner.statuses);
            match statuses.get_mut(chat_id) {
                Some(row) if row.limited.is_some() => return true,
                Some(row) if is_active(row) || row.last_turn_outcome.is_some_and(|o| o.finished_at > mark.limit.since + chrono::Duration::seconds(5)) => {
                    drop(statuses);
                    self.inner.journal.clear_limited(chat_id);
                    return false;
                }
                Some(row) => {
                    row.limited = Some(mark.limit.clone());
                    row.updated_at = Utc::now();
                }
                None => {
                    statuses.insert(
                        chat_id.to_owned(),
                        Session {
                            last_completed_turn: None,
                            chat_id: chat_id.to_owned(),
                            device_id: self.inner.device_id.clone(),
                            status: SessionStatus::Errored,
                            started_at: None,
                            updated_at: Utc::now(),
                            last_turn_outcome: Some(zeron_proto::NativeTurnOutcome {
                                status: DoneStatus::Errored,
                                finished_at: mark.limit.since,
                            }),
                            terminal_activity: None,
                            running_subagents: 0,
                            background_tasks: Vec::new(),
                            waiting: None,
                            activity: None,
                            todo: None,
                            limited: Some(mark.limit.clone()),
                        },
                    );
                }
            }
            let row = statuses.get(chat_id).cloned();
            let mut list: Vec<Session> = statuses.values().cloned().collect();
            list.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
            self.inner.sessions_tx.send_replace(list);
            row
        };
        if self.inner.activity_tx.get().is_none()
            && let (Some(ws), Some(session)) = (self.inner.workspace(), session)
        {
            ws.record_session(&session);
        }
        true
    }

    /// The limits marked beside the journal (see [`RunJournal::note_limited`]).
    pub(crate) fn limit_marks(&self) -> Vec<LimitMark> {
        self.inner.journal.limited_marks().unwrap_or_default()
    }
}
