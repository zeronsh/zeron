//! Turns the provider's usage limit stopped: recognizing the stop as the run
//! ends, the auto-resume state its session row carries (see
//! [`zeron_proto::AutoResume`]), and continuing the stopped turn once the
//! limit has reset. When to resume is [`crate::auto_resume::AutoResumer`]'s
//! call; this side keeps the row and starts the continuation.

use super::*;
use chrono::DateTime;
use zeron_proto::auto_resume::{
    MAX_ATTEMPTS, check_backoff, limit_never_resets, limit_stop_text, reset_time_in,
};
use zeron_proto::{AutoResume, AutoResumePhase};

/// What [`SessionsEngine::resume_limited`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitResume {
    /// The continuation started.
    Started,
    /// The chat moved on (a newer message or entry, a live run, a Stop) or
    /// has nothing to continue: the stop is over.
    MovedOn,
    /// The stop changed meanwhile (toggled off, replaced, already resuming):
    /// left as it is.
    NotNow,
}

/// Whether a turn of `harness` that failed with `parts` was stopped by the
/// account's usage limit, and when that resets. Claude Code and Codex say so
/// themselves (`signal`, from [`AgentEvent::LimitReached`]) — their error
/// text reads the same for throttles and limits no reset clears, so it never
/// decides. Other harnesses only have their error text. A stop needs an
/// error card to show it on, so a turn without an error part is none; and
/// no harness's word makes a stop of a limit no reset clears
/// ([`limit_never_resets`]). A reset time not after `now` says nothing.
pub(super) fn limit_stop(
    harness: HarnessId,
    signal: Option<Option<DateTime<Utc>>>,
    parts: &[MessagePart],
    now: DateTime<Utc>,
) -> Option<Option<DateTime<Utc>>> {
    let errors: Vec<&str> = parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Error { message, .. } => Some(message.as_str()),
            _ => None,
        })
        .collect();
    if errors.is_empty()
        || errors
            .iter()
            .any(|message| limit_never_resets(message, now))
    {
        return None;
    }
    let signals = matches!(harness, HarnessId::ClaudeCode | HarnessId::Codex);
    let worded = !signals && errors.iter().any(|message| limit_stop_text(message));
    if signal.is_none() && !worded {
        return None;
    }
    let ahead = |at: &DateTime<Utc>| *at > now;
    Some(signal.flatten().filter(ahead).or_else(|| {
        errors
            .iter()
            .find_map(|message| reset_time_in(message, now))
            .filter(ahead)
    }))
}

/// A turn starts in a chat whose row holds `state`. `ours` is the stop the
/// starting turn continues, when it is the host's continuation (its revival
/// carried that stop): it makes the stop `Resumed` (a start retry keeps
/// it). Any other turn (the user's message, a parent's, the next one after a
/// resumed turn) means the chat moved on.
pub(super) fn turn_started(state: &mut Option<AutoResume>, now: DateTime<Utc>, ours: Option<&str>) {
    match state {
        Some(resume)
            if ours == Some(resume.entry_id.as_str())
                && matches!(
                    resume.phase,
                    AutoResumePhase::Resuming | AutoResumePhase::Resumed
                ) =>
        {
            if resume.phase == AutoResumePhase::Resuming {
                resume.phase = AutoResumePhase::Resumed;
                resume.resumed_at = Some(now);
            }
        }
        Some(_) => *state = None,
        None => {}
    }
}

/// The stop a turn ending on the limit at `entry_id` records, after
/// `previous` (the row's stop before it). A stop right after a continuation
/// counts on from it: its resume waits at least that attempt's backoff,
/// however soon the limit says it resets, and past [`MAX_ATTEMPTS`] the host
/// gives up — a limit that keeps taking the continuation straight back is
/// not one that clears by itself, whatever it said.
pub(super) fn new_stop(
    previous: Option<&AutoResume>,
    entry_id: &str,
    enabled: bool,
    resets_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> AutoResume {
    let attempt = previous
        .filter(|previous| previous.phase == AutoResumePhase::Resumed)
        .map_or(0, |previous| previous.attempt.saturating_add(1));
    let gave_up = attempt >= MAX_ATTEMPTS;
    AutoResume {
        entry_id: entry_id.to_owned(),
        enabled,
        phase: if gave_up {
            AutoResumePhase::GaveUp
        } else {
            AutoResumePhase::Waiting
        },
        resume_at: resets_at.filter(|_| !gave_up).map(|reset| {
            if attempt > 0 {
                reset.max(now + check_backoff(attempt))
            } else {
                reset
            }
        }),
        stopped_at: now,
        resumed_at: None,
        attempt,
    }
}

/// What the continuation sends when the provider's session already holds
/// the stopped turn.
fn limit_continuation(original: &str) -> String {
    format!(
        "Your previous turn stopped because the provider's usage limit was reached. The \
         limit has reset now: continue where you left off. Anything you already completed, \
         such as edits, commands or commits, is done: check before repeating it. The request \
         is below for reference; if it is not in your history yet, handle it now.\n\n\
         <request>\n{original}\n</request>"
    )
}

/// The continuation for a stopped message that had no text (images only):
/// the provider's session holds it, and there is no request to repeat.
const LIMIT_CONTINUATION_BARE: &str = "Your previous turn stopped because the provider's \
     usage limit was reached. The limit has reset now: continue where you left off. Anything \
     you already completed, such as edits, commands or commits, is done: check before \
     repeating it.";

impl Inner {
    /// The run ended `Errored` because the usage limit stopped `entry_id`'s
    /// turn: settle the row with the stop's auto-resume state, defaulting to
    /// the device setting. Without auto-resume wired, a plain error.
    pub(super) fn set_status_limited(
        &self,
        chat_id: &str,
        outcome: Option<DoneStatus>,
        entry_id: &str,
        resets_at: Option<DateTime<Utc>>,
    ) {
        let Some(default) = self.auto_resume_default.get() else {
            self.set_status_with_completion(chat_id, SessionStatus::Errored, false, None, outcome);
            return;
        };
        let previous = lock(&self.statuses)
            .get(chat_id)
            .and_then(|row| row.auto_resume.clone());
        let stop = new_stop(
            previous.as_ref(),
            entry_id,
            default.load(std::sync::atomic::Ordering::Relaxed),
            resets_at,
            Utc::now(),
        );
        let attempt = stop.attempt;
        tracing::info!(chat = %chat_id, entry = %entry_id, ?resets_at, enabled = stop.enabled,
            attempt, phase = ?stop.phase, "usage limit stopped the turn");
        self.update_status(
            chat_id,
            SessionStatus::Errored,
            false,
            None,
            outcome,
            Some(stop),
            None,
        );
    }

    /// Drop `chat_id`'s limit stop unless it already resumed: the user took
    /// the chat over (a Stop, an archive, a delete). With `entry_id`, only
    /// that stop — a newer one stays.
    pub(super) fn cancel_auto_resume(&self, chat_id: &str, entry_id: Option<&str>) {
        self.update_auto_resume(chat_id, |state| {
            if state.as_ref().is_some_and(|resume| {
                resume.phase != AutoResumePhase::Resumed
                    && entry_id.is_none_or(|entry| entry == resume.entry_id)
            }) {
                *state = None;
            }
        });
    }

    /// Change `chat_id`'s auto-resume state in place, publishing the row like
    /// any transition when it changed. Returns the state after. A chat
    /// without a row has none.
    fn update_auto_resume(
        &self,
        chat_id: &str,
        change: impl FnOnce(&mut Option<AutoResume>),
    ) -> Option<AutoResume> {
        let (session, after) = {
            let mut statuses = lock(&self.statuses);
            let entry = statuses.get_mut(chat_id)?;
            let before = entry.auto_resume.clone();
            change(&mut entry.auto_resume);
            if entry.auto_resume == before {
                return before;
            }
            entry.updated_at = Utc::now();
            let session = entry.clone();
            let mut list: Vec<Session> = statuses.values().cloned().collect();
            list.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
            self.sessions_tx.send_replace(list);
            let after = session.auto_resume.clone();
            (session, after)
        };
        if self.activity_tx.get().is_none()
            && let Some(ws) = self.workspace()
        {
            ws.record_session(&session);
        }
        after
    }
}

impl SessionsEngine {
    /// Wire the device's auto-resume default (called once at engine
    /// assembly): from then on limit stops are recorded on their rows.
    pub fn set_auto_resume_default(&self, enabled: Arc<std::sync::atomic::AtomicBool>) {
        let _ = self.inner.auto_resume_default.set(enabled);
    }

    /// Every row holding a limit stop.
    pub fn auto_resumes(&self) -> Vec<Session> {
        lock(&self.inner.statuses)
            .values()
            .filter(|row| row.auto_resume.is_some())
            .cloned()
            .collect()
    }

    /// Change `chat_id`'s limit stop in place (see [`Inner::update_auto_resume`]).
    pub fn update_auto_resume(
        &self,
        chat_id: &str,
        change: impl FnOnce(&mut Option<AutoResume>),
    ) -> Option<AutoResume> {
        self.inner.update_auto_resume(chat_id, change)
    }

    /// Drop `chat_id`'s limit stop `entry_id` (any, when `None`) unless it
    /// already resumed.
    pub fn cancel_auto_resume(&self, chat_id: &str, entry_id: Option<&str>) {
        self.inner.cancel_auto_resume(chat_id, entry_id);
    }

    /// Bring back a limit stop saved before the engine restarted: its row as
    /// it was then. A chat that has a row again since (it ran) moved on.
    pub fn restore_auto_resume(&self, row: Session) {
        let session = {
            let mut statuses = lock(&self.inner.statuses);
            if statuses.contains_key(&row.chat_id) || row.auto_resume.is_none() {
                return;
            }
            let mut row = row;
            row.device_id = self.inner.device_id.clone();
            row.updated_at = Utc::now();
            statuses.insert(row.chat_id.clone(), row.clone());
            let mut list: Vec<Session> = statuses.values().cloned().collect();
            list.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
            self.inner.sessions_tx.send_replace(list);
            row
        };
        if self.inner.activity_tx.get().is_none()
            && let Some(ws) = self.inner.workspace()
        {
            ws.record_session(&session);
        }
    }

    /// Continue the turn the usage limit stopped at `entry_id`, now that the
    /// limit has reset: the stopped turn's user message goes back to the
    /// provider as a revival (no new message in the transcript), sending a
    /// short continuation when the provider's session already holds the
    /// turn — which carries on whatever the turn had running, its own
    /// subagents included. Only from the stop as it is now: enabled, waiting
    /// and still `entry_id`; it turns `Resuming`, then `Resumed` once the
    /// continuation's run is the chat's. A Stop, a message, any entry after
    /// the stop or a live run since make it [`LimitResume::MovedOn`].
    pub async fn resume_limited(
        &self,
        chat_id: &str,
        entry_id: &str,
    ) -> Result<LimitResume, EngineError> {
        // A Stop from here on ends the continuation before it starts.
        let stops = self.inner.user_stop_count(chat_id);
        let host = self
            .inner
            .doc_host()
            .ok_or_else(|| EngineError::Other("doc host not wired into sessions engine".into()))?;
        if self.has_run(chat_id) {
            return Ok(LimitResume::MovedOn);
        }
        let entries = host.open(chat_id)?.doc().read_entries()?;
        // The stopped reply must be the transcript's last word.
        let Some(stop) = entries.iter().position(|entry| entry.id == entry_id) else {
            return Ok(LimitResume::MovedOn);
        };
        if stop + 1 != entries.len() {
            return Ok(LimitResume::MovedOn);
        }
        let Some(message) = entries[..stop]
            .iter()
            .rev()
            .find(|entry| entry.role == MessageRole::User)
        else {
            return Ok(LimitResume::MovedOn);
        };
        let text = message.parts.iter().find_map(|part| match part {
            MessagePart::Text { text, .. } if !text.trim().is_empty() => Some(text.clone()),
            _ => None,
        });
        let (prompt, continuation) = match text {
            Some(text) => {
                let continuation = limit_continuation(&text);
                (text, continuation)
            }
            // Images only: the provider's session has them; a session
            // started over gets the bare continuation.
            None => (
                LIMIT_CONTINUATION_BARE.to_owned(),
                LIMIT_CONTINUATION_BARE.to_owned(),
            ),
        };
        let Some((mut request, harness_id)) = self.revival_request(chat_id, &prompt) else {
            tracing::warn!(chat = %chat_id, "limit auto-resume skipped: no run config");
            return Ok(LimitResume::MovedOn);
        };
        request.resume = None; // dispatch re-injects the remembered session
        request.attachments = Vec::new();
        let claimed = self.inner.update_auto_resume(chat_id, |state| {
            if let Some(resume) = state.as_mut().filter(|resume| {
                resume.entry_id == entry_id
                    && resume.enabled
                    && resume.phase == AutoResumePhase::Waiting
            }) {
                resume.phase = AutoResumePhase::Resuming;
            }
        });
        if !claimed.is_some_and(|resume| {
            resume.entry_id == entry_id && resume.phase == AutoResumePhase::Resuming
        }) {
            return Ok(LimitResume::NotNow);
        }
        let user_id = message.id.clone();
        lock(&self.inner.revivals).insert(
            chat_id.to_owned(),
            Revival {
                message_id: user_id.clone(),
                continuation: Some(continuation),
                limit_entry: Some(entry_id.to_owned()),
            },
        );
        match self
            .dispatch_with(
                chat_id,
                harness_id,
                request,
                Some(user_id.clone()),
                false,
                0,
                Some(stops),
            )
            .await
        {
            // Empty: dispatch found the chat had moved on, or was stopped.
            Ok(run_id) if run_id.is_empty() => {
                self.inner.drop_revival(chat_id, &user_id);
                Ok(LimitResume::MovedOn)
            }
            Ok(_) => {
                tracing::info!(chat = %chat_id, "auto-resumed a turn the usage limit stopped");
                Ok(LimitResume::Started)
            }
            Err(err) => {
                self.inner.drop_revival(chat_id, &user_id);
                self.inner.update_auto_resume(chat_id, |state| {
                    if let Some(resume) = state
                        .as_mut()
                        .filter(|r| r.entry_id == entry_id && r.phase == AutoResumePhase::Resuming)
                    {
                        resume.phase = AutoResumePhase::Waiting;
                    }
                });
                Err(err)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(message: &str) -> MessagePart {
        MessagePart::Error {
            id: "e0".into(),
            message: message.into(),
        }
    }

    #[test]
    fn a_limit_stop_needs_the_signal_or_the_wording_and_an_error_card() {
        let now = Utc::now();
        let at = now + chrono::Duration::hours(2);
        let claude = HarnessId::ClaudeCode;
        let pi = HarnessId::Pi;
        // The harness's own word, with its time.
        assert_eq!(
            limit_stop(
                claude,
                Some(Some(at)),
                &[error("You've hit your 5-hour limit")],
                now
            ),
            Some(Some(at))
        );
        // Its word without a time: the text's.
        assert_eq!(
            limit_stop(
                claude,
                Some(None),
                &[error("You've hit your limit · resets 3pm (UTC)")],
                now
            ),
            Some(reset_time_in("resets 3pm (UTC)", now))
        );
        // Claude's and Codex's text alone never decides: a throttle reads
        // the same.
        assert_eq!(
            limit_stop(claude, None, &[error("Claude usage limit reached")], now),
            None
        );
        assert_eq!(
            limit_stop(
                HarnessId::Codex,
                None,
                &[error("Codex usage limit reached")],
                now
            ),
            None
        );
        // Other harnesses: the wording, with its time.
        assert_eq!(
            limit_stop(
                pi,
                None,
                &[error("Usage limit reached — it resets in 45m.")],
                now
            ),
            Some(Some(now + chrono::Duration::minutes(45)))
        );
        assert_eq!(
            limit_stop(pi, None, &[error("429 Too Many Requests")], now),
            None
        );
        assert_eq!(
            limit_stop(pi, None, &[error("Harness exited with status 1")], now),
            None
        );
        // No card to show it on.
        assert_eq!(limit_stop(claude, Some(Some(at)), &[], now), None);
        // A reset already past says nothing about when: the host asks.
        assert_eq!(
            limit_stop(
                claude,
                Some(Some(now - chrono::Duration::minutes(5))),
                &[error("You've hit your 5-hour limit")],
                now
            ),
            Some(None)
        );
        // No signal makes a stop of a limit no reset clears.
        for never in [
            "API Error: Server is temporarily limiting requests (not your usage limit)",
            "You've hit your usage limit · contact your admin to increase it",
            "You're out of usage credits",
        ] {
            assert_eq!(
                limit_stop(claude, Some(Some(at)), &[error(never)], now),
                None,
                "{never}"
            );
        }
    }

    #[test]
    fn a_turn_start_resumes_or_moves_on() {
        let now = Utc::now();
        let stop = |phase| {
            Some(AutoResume {
                entry_id: "a".into(),
                enabled: true,
                phase,
                resume_at: None,
                stopped_at: now,
                resumed_at: None,
                attempt: 0,
            })
        };
        let mut resuming = stop(AutoResumePhase::Resuming);
        turn_started(&mut resuming, now, Some("a"));
        let resumed = resuming.clone().expect("kept");
        assert_eq!(resumed.phase, AutoResumePhase::Resumed);
        assert_eq!(resumed.resumed_at, Some(now));
        // The continuation's start retry keeps it.
        turn_started(&mut resuming, now, Some("a"));
        assert_eq!(resuming, Some(resumed));
        // Any other turn: the chat moved on, even one starting while the
        // host was resuming.
        for phase in [
            AutoResumePhase::Waiting,
            AutoResumePhase::Resuming,
            AutoResumePhase::Resumed,
            AutoResumePhase::GaveUp,
        ] {
            let mut state = stop(phase);
            turn_started(&mut state, now, None);
            assert_eq!(state, None, "{phase:?}");
            let mut state = stop(phase);
            turn_started(&mut state, now, Some("another stop"));
            assert_eq!(state, None, "{phase:?}");
        }
    }

    #[test]
    fn a_stop_after_a_continuation_backs_off_and_finally_gives_up() {
        let now = Utc::now();
        let soon = now + chrono::Duration::minutes(1);
        let first = new_stop(None, "a", true, Some(soon), now);
        assert_eq!(
            (first.phase, first.attempt, first.resume_at),
            (AutoResumePhase::Waiting, 0, Some(soon))
        );
        // Not after a continuation (another turn, or still waiting): fresh.
        assert_eq!(
            new_stop(Some(&first), "b", true, Some(soon), now).attempt,
            0
        );
        let mut previous = AutoResume {
            phase: AutoResumePhase::Resumed,
            ..first
        };
        let again = new_stop(Some(&previous), "b", true, Some(soon), now);
        assert_eq!(again.attempt, 1);
        assert_eq!(
            again.resume_at,
            Some(now + check_backoff(1)),
            "not the minute the limit claims"
        );
        // A later reset still wins.
        let later = now + chrono::Duration::hours(3);
        assert_eq!(
            new_stop(Some(&previous), "b", true, Some(later), now).resume_at,
            Some(later)
        );
        previous.attempt = MAX_ATTEMPTS - 1;
        let done = new_stop(Some(&previous), "c", true, Some(soon), now);
        assert_eq!(
            (done.phase, done.resume_at),
            (AutoResumePhase::GaveUp, None)
        );
        assert!(!done.pending());
    }

    #[test]
    fn the_continuation_carries_the_request() {
        let nudge = limit_continuation("refactor the parser");
        assert!(nudge.contains("usage limit was reached"));
        assert!(nudge.ends_with("<request>\nrefactor the parser\n</request>"));
    }
}
