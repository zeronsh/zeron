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

/// Most links a filed report keeps.
const LINKS_MAX: usize = 10;

impl DocHost {
    /// `report` as this host files it, everything re-derived from the rows:
    /// it must come from a thread in `chat`'s own crew, its orchestrator is
    /// `chat`, its item one on `chat`'s board (else the thread's own), and
    /// its text is capped. `None` when it isn't this crew's.
    fn admit_report(&self, chat: &str, report: &Report) -> Option<Report> {
        use zeron_proto::orchestration::prompt_text::{BLOCK_MAX, LABEL_MAX, SUMMARY_MAX, TARGET_MAX, one_line};
        let ws = self.workspace()?;
        let (member, item) = ws.read_orchestration_with(|doc| {
            let member = doc.crew_member(&report.from_chat);
            let item = report.item_id.as_deref().and_then(|id| doc.work_item(id));
            (member, item)
        });
        let member = member.filter(|m| m.orchestrator_id == chat)?;
        let item_id = item
            .filter(|i| i.orchestrator_id.as_deref() == Some(chat))
            .map(|i| i.id)
            .or(member.item_id);
        let summary = one_line(&report.summary, SUMMARY_MAX);
        Some(Report {
            id: one_line(&report.id, LABEL_MAX),
            orchestrator_id: chat.to_owned(),
            from_chat: member.chat_id,
            item_id,
            state: report.state,
            summary: if summary.is_empty() { "Report".into() } else { summary },
            detail: report.detail.chars().take(BLOCK_MAX).collect(),
            links: report
                .links
                .iter()
                .take(LINKS_MAX)
                .map(|l| zeron_proto::orchestration::WorkLink {
                    kind: l.kind,
                    label: one_line(&l.label, LABEL_MAX),
                    target: one_line(&l.target, TARGET_MAX),
                })
                .collect(),
            progress: report.progress.filter(|p| p.is_finite()).map(|p| p.clamp(0.0, 1.0)),
            at: report.at,
        })
    }

    /// The wake turn's run: as the orchestrator's chat is set (its own
    /// harness, model, sandbox, access and folder), never what an earlier
    /// run carried, with the same sanitizing an agent's run gets.
    fn wake_request(&self, sessions: &SessionsEngine, chat: &str, prompt: String) -> Option<zeron_proto::RunRequest> {
        let ws = self.workspace()?;
        let row = ws.chat(chat).ok().flatten()?;
        let mut request = self.request_from_chat_row(chat, &prompt)?;
        if request.cwd.trim().is_empty() {
            request.cwd = row
                .space_id
                .as_deref()
                .and_then(|id| ws.space(id).ok().flatten())
                .map(|space| space.path)
                .or_else(|| sessions.last_request(chat).map(|r| r.cwd))
                .unwrap_or_else(|| "~".into());
        }
        request.attachments = Vec::new();
        request.resume = None;
        request.worktree = None;
        request.env = Default::default();
        let mut value = serde_json::to_value(&request).ok()?;
        crate::rpc::run_as_chat_is_set(
            &mut value,
            Some(&row),
            (zeron_proto::SandboxLevel::WorkspaceWrite, zeron_proto::AccessMode::Supervised),
        );
        let mut request: zeron_proto::RunRequest = serde_json::from_value(value).ok()?;
        request.prompt = prompt;
        Some(request)
    }

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
        let Some(report) = self.admit_report(&chat, report) else {
            return Ok((SessionCommandStatus::Rejected, Some("not from this orchestrator's crew".into())));
        };
        let report = &report;
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
                match self.wake_request(&sessions, &chat, prompt) {
                    Some(request) => {
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
