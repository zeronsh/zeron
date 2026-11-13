//! An orchestrator's crew reports, on the engine that hosts it.
//!
//! A report arrives as a `crewReport` command in the orchestrator's doc (any
//! device queues it: `orchestration::report::file_report`). Here it is filed
//! in the transcript: every report since the orchestrator's last turn goes
//! into one "Crew updates" message (`MessagePart::CrewReports`), extended in
//! place, never a user turn. While the orchestrator works, reports wait and
//! are filed when its turn ends, so they never land inside a streaming turn.
//!
//! Waking: a report whose state wakes (`ReportState::wakes`) wakes an idle
//! orchestrator after [`WAKE_DEBOUNCE`], so the reports that follow join it,
//! and a working one when its turn ends. One wake turn carries every unread
//! report; its prompt is generated (`reports_prompt`) and its transcript
//! entry is the Crew updates message itself (the run's message id).

use std::time::Duration;

use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionCommandPayload, SessionCommandStatus, SessionMessageEntry};
use zeron_proto::orchestration::{Report, reports_prompt};

use super::{ChatDocHandle, DocHost, lock};
use crate::sessions::SessionsEngine;
use crate::{EngineError, new_id, now_ms};

/// How long an idle orchestrator waits after a waking report for more to
/// join its wake.
pub const WAKE_DEBOUNCE: Duration = Duration::from_secs(15);

/// Every orchestrator this engine hosts that has reports in flight.
#[derive(Default)]
pub(super) struct Inbox {
    chats: std::sync::Mutex<std::collections::HashMap<String, Pending>>,
    /// [`WAKE_DEBOUNCE`] unless a test set it (ms; 0 = default).
    debounce_ms: std::sync::atomic::AtomicU64,
}

#[derive(Default)]
struct Pending {
    /// The Crew updates message still open (the transcript's last entry when
    /// it was written) and the reports it shows.
    open: Option<(String, Vec<Report>)>,
    /// Filed while the orchestrator worked: written when its turn ends.
    held: Vec<Report>,
    /// What its harness hasn't been given yet.
    unread: Vec<Report>,
    /// Some unread report wakes it.
    wake: bool,
    /// When an idle orchestrator's wake is due.
    deadline: Option<tokio::time::Instant>,
    /// A task is waiting to settle this chat.
    task: bool,
}

/// A Crew updates message's parts: the card, and a line per report for
/// builds that don't know it.
fn crew_parts(reports: &[Report]) -> Vec<MessagePart> {
    vec![
        MessagePart::Text {
            id: "t0".into(),
            text: zeron_doc::crew_reports_fallback_text(reports),
        },
        MessagePart::CrewReports {
            id: "crew".into(),
            reports: reports.to_vec(),
        },
    ]
}

/// File `reports` in the transcript: into the open message while it is
/// still the last entry, else as a new one.
fn append(handle: &ChatDocHandle, device_id: &str, pending: &mut Pending, reports: Vec<Report>) -> Result<(), EngineError> {
    if reports.is_empty() {
        return Ok(());
    }
    let last = handle.doc.read_entries_shared()?.last().map(|e| e.id.clone());
    if let Some((id, shown)) = pending.open.as_mut()
        && last.as_deref() == Some(id.as_str())
    {
        shown.extend(reports);
        handle.doc.replace_message_parts(id, &crew_parts(shown))?;
        handle.publish_messages();
        return Ok(());
    }
    let id = format!("crew-{}", new_id());
    handle.doc.push_message(&SessionMessageEntry {
        id: id.clone(),
        role: MessageRole::User,
        parts: crew_parts(&reports),
        created_at: now_ms(),
        device_id: device_id.to_owned(),
        status: Some(MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    })?;
    handle.publish_messages();
    pending.open = Some((id, reports));
    Ok(())
}

impl DocHost {
    /// How long an idle orchestrator waits for more reports before it wakes
    /// ([`WAKE_DEBOUNCE`] by default). For tests.
    #[doc(hidden)]
    pub fn set_crew_wake_debounce(&self, debounce: Duration) {
        self.inner
            .crew_inbox
            .debounce_ms
            .store(debounce.as_millis().max(1) as u64, std::sync::atomic::Ordering::Relaxed);
    }

    fn crew_wake_debounce(&self) -> Duration {
        match self.inner.crew_inbox.debounce_ms.load(std::sync::atomic::Ordering::Relaxed) {
            0 => WAKE_DEBOUNCE,
            ms => Duration::from_millis(ms),
        }
    }

    /// The `crewReport` command: file `report` in this orchestrator's
    /// transcript and wake it when it should. Only a report from a thread in
    /// this orchestrator's own crew is filed.
    pub(super) fn file_crew_report(
        &self,
        sessions: &SessionsEngine,
        handle: &std::sync::Arc<ChatDocHandle>,
        report: &Report,
    ) -> Result<(SessionCommandStatus, Option<String>), EngineError> {
        let chat = handle.chat_id.clone();
        if report.orchestrator_id != chat {
            return Ok((SessionCommandStatus::Rejected, Some("a report for another orchestrator".into())));
        }
        let member = self
            .workspace()
            .and_then(|ws| ws.read_orchestration_with(|doc| doc.crew_member(&report.from_chat)));
        if member.is_none_or(|m| m.orchestrator_id != chat) {
            return Ok((SessionCommandStatus::Rejected, Some("not from this orchestrator's crew".into())));
        }
        let debounce = self.crew_wake_debounce();
        let spawn = {
            let mut chats = lock(&self.inner.crew_inbox.chats);
            let pending = chats.entry(chat.clone()).or_default();
            // A redelivered command files nothing twice.
            let known = |r: &Report| r.id == report.id;
            if pending.unread.iter().any(known)
                || pending.held.iter().any(known)
                || pending.open.as_ref().is_some_and(|(_, shown)| shown.iter().any(known))
            {
                return Ok((SessionCommandStatus::Applied, Some("already filed".into())));
            }
            let working = sessions.turn_in_flight(&chat);
            if working {
                pending.held.push(report.clone());
            } else {
                append(handle, self.device_id(), pending, vec![report.clone()])?;
            }
            pending.unread.push(report.clone());
            if report.state.wakes() {
                pending.wake = true;
                if !working && pending.deadline.is_none() {
                    pending.deadline = Some(tokio::time::Instant::now() + debounce);
                }
            }
            let spawn = (pending.wake || !pending.held.is_empty()) && !pending.task;
            if spawn {
                pending.task = true;
            }
            spawn
        };
        if spawn {
            let host = self.clone();
            let sessions = sessions.clone();
            self.spawn_worker(async move { host.settle_crew_reports(sessions, chat).await });
        }
        Ok((SessionCommandStatus::Applied, None))
    }

    /// Wait out the debounce and the orchestrator's turn, then file what
    /// waited and wake it when a report asked to.
    async fn settle_crew_reports(&self, sessions: SessionsEngine, chat: String) {
        let mut statuses = sessions.watch_sessions();
        loop {
            let deadline = lock(&self.inner.crew_inbox.chats).get(&chat).and_then(|p| p.deadline);
            if let Some(deadline) = deadline {
                tokio::time::sleep_until(deadline).await;
            }
            while sessions.turn_in_flight(&chat) {
                if statuses.changed().await.is_err() {
                    lock(&self.inner.crew_inbox.chats).remove(&chat);
                    return;
                }
            }
            let handle = match self.open(&chat) {
                Ok(handle) => handle,
                Err(err) => {
                    tracing::warn!(chat = %chat, error = %err, "crew reports: orchestrator doc won't open");
                    lock(&self.inner.crew_inbox.chats).remove(&chat);
                    return;
                }
            };
            let mut chats = lock(&self.inner.crew_inbox.chats);
            let Some(pending) = chats.get_mut(&chat) else { return };
            // A turn started while we woke (the person typed): wait for it.
            if sessions.turn_in_flight(&chat) {
                continue;
            }
            let held = std::mem::take(&mut pending.held);
            if let Err(err) = append(&handle, self.device_id(), pending, held) {
                tracing::warn!(chat = %chat, error = %err, "crew reports: transcript write failed");
            }
            if pending.wake && !pending.unread.is_empty() {
                let prompt = reports_prompt(&pending.unread);
                let message_id = pending
                    .open
                    .as_ref()
                    .map(|(id, _)| id.clone())
                    .unwrap_or_else(|| format!("crew-{}", new_id()));
                let request = sessions
                    .last_request(&chat)
                    .or_else(|| self.request_from_chat_row(&chat, &prompt));
                match request {
                    Some(mut request) => {
                        request.prompt = prompt;
                        request.resume = None;
                        request.attachments = Vec::new();
                        request.worktree = None;
                        if let Err(err) =
                            self.queue_command(&chat, SessionCommandPayload::Run { request, message_id })
                        {
                            tracing::warn!(chat = %chat, error = %err, "crew reports: wake failed");
                        }
                    }
                    None => tracing::warn!(chat = %chat, "crew reports: no run config to wake with"),
                }
                pending.unread.clear();
                pending.open = None;
                pending.wake = false;
            }
            pending.deadline = None;
            pending.task = false;
            if pending.unread.is_empty() && pending.open.is_none() {
                chats.remove(&chat);
            }
            return;
        }
    }
}
