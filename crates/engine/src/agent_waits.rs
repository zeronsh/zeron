//! Waiting without stopping: an agent's questions to the person, and its
//! watches on other chats' turns. Both end in a message the engine delivers
//! into the agent's own chat, later, through the composer's machinery: a
//! `Steer` command, which steers a running turn and starts a new one when
//! the chat is idle (`DocHost::deliver_prompt`).
//!
//! - Questions (`ask_question`, [`zeron_proto::questions`]) are registry
//!   rows, so every device shows them and any can answer. Answering on a
//!   device queues the delivery from that device; the chat doc carries the
//!   command to whichever device hosts the chat. A blocking question is
//!   answered inside the agent's tool call instead, until its MCP server
//!   lets go of it ([`AgentWaits::release`]).
//! - Turn watches (`wait_for_turn` without `blocking`, `send_message` with
//!   `notify`) are this device's: the agent's MCP server runs here, and so
//!   does the watch. They are kept in `turn-watches.json` under the store
//!   root, so a restart resumes them: the turn-end edge is a baseline
//!   comparison ([`TurnProgress`]) that a missed moment cannot lose.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use zeron_doc::SessionCommandPayload;
use zeron_mcp::{RenderOptions, TurnOutcome, TurnProgress, render_entries, session_for};
use zeron_proto::questions::{self, AgentQuestion, QuestionStatus};
use zeron_proto::{Chat, Session};

use crate::doc_host::DocHost;
use crate::sessions::SessionsEngine;
use crate::workspace_host::WorkspaceHost;
use crate::{EngineError, new_id, now_ms};

/// A watch still unsettled after this long is dropped, with a note.
const WATCH_MAX_AGE_MS: i64 = 24 * 60 * 60 * 1000;
/// How long a finished turn's reply may trail its session row (another
/// device's transcript syncs separately).
const REPLY_GRACE: Duration = Duration::from_secs(15);
/// Longest reply excerpt a notification carries.
const EXCERPT_CHARS: usize = 1_500;
/// Re-evaluation cadence besides session changes (expiry, stale rows).
const SWEEP: Duration = Duration::from_secs(30);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One chat waiting to hear that another chat's turn ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnWatch {
    pub id: String,
    /// Where the notification goes.
    pub watcher_chat_id: String,
    /// The chat whose turn is awaited.
    pub target_chat_id: String,
    pub progress: TurnProgress,
    /// Transcript messages that predate the awaited turn: its reply is the
    /// assistant messages not among them.
    #[serde(default)]
    pub start_message_ids: Vec<String>,
    pub created_at_ms: i64,
    /// Settled, the notification not yet queued.
    #[serde(default)]
    pub settled: Option<TurnOutcome>,
}

#[derive(Clone)]
pub struct AgentWaits {
    inner: Arc<Inner>,
}

struct Inner {
    doc_host: DocHost,
    workspace: WorkspaceHost,
    sessions: SessionsEngine,
    /// `turn-watches.json`; `None` keeps watches in memory (tests).
    path: Option<PathBuf>,
    watches: Mutex<Vec<TurnWatch>>,
    /// Watches settled and being delivered: not settled twice.
    delivering: Mutex<std::collections::HashSet<String>>,
    wake: Arc<Notify>,
    started: std::sync::Once,
}

impl AgentWaits {
    pub fn new(
        doc_host: DocHost,
        workspace: WorkspaceHost,
        sessions: SessionsEngine,
        path: Option<PathBuf>,
    ) -> Self {
        let watches: Vec<TurnWatch> = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let resume = !watches.is_empty();
        let waits = Self {
            inner: Arc::new(Inner {
                doc_host,
                workspace,
                sessions,
                path,
                watches: Mutex::new(watches),
                delivering: Mutex::default(),
                wake: Arc::new(Notify::new()),
                started: std::sync::Once::new(),
            }),
        };
        if resume {
            tracing::info!("agent waits: resuming turn watches");
            waits.start();
        }
        waits
    }

    // ── questions ───────────────────────────────────────────────────────────

    /// Record a question from `chat_id`'s agent.
    pub fn ask(
        &self,
        chat_id: &str,
        question: &str,
        options: &[String],
        context: Option<&str>,
        blocking: bool,
    ) -> Result<AgentQuestion, EngineError> {
        // A Cloud machine's registry writes are limited to the rows it hosts
        // (edge/src/registry-runner.ts): a question would never leave it.
        if crate::runner::is_cloud_platform() {
            return Err(EngineError::Other(
                "Questions can't reach the person from a Cloud machine yet: ask in your reply instead".into(),
            ));
        }
        let (question, options, context) =
            questions::validate_question(question, options, context).map_err(EngineError::Other)?;
        if self.inner.workspace.chat(chat_id)?.is_none() {
            return Err(EngineError::Other(format!("no chat {chat_id}")));
        }
        let asked = AgentQuestion {
            id: format!("q-{}", new_id()),
            chat_id: chat_id.to_owned(),
            question,
            options,
            context,
            blocking,
            status: QuestionStatus::Pending,
            answer: None,
            created_at: chrono::Utc::now(),
            resolved_at: None,
            resolved_by: None,
            delivered_at: None,
        };
        self.inner.workspace.add_question(&asked)?;
        Ok(asked)
    }

    fn question(&self, id: &str) -> Result<AgentQuestion, EngineError> {
        self.inner
            .workspace
            .question(id)
            .ok_or_else(|| EngineError::Other(format!("no question {id}")))
    }

    /// The person's answer. Delivered into the asking chat unless the agent
    /// waits for it inside its tool call.
    pub fn answer(&self, id: &str, answer: &str) -> Result<AgentQuestion, EngineError> {
        let answer = questions::validate_answer(answer).map_err(EngineError::Other)?;
        self.resolve(id, QuestionStatus::Answered, Some(&answer))
    }

    /// The person closes it unanswered. The agent is told.
    pub fn dismiss(&self, id: &str) -> Result<AgentQuestion, EngineError> {
        self.resolve(id, QuestionStatus::Dismissed, None)
    }

    /// The asking agent withdraws it. `caller` is the agent's chat, when an
    /// agent calls: only its own questions.
    pub fn cancel(&self, id: &str, caller: Option<&str>) -> Result<AgentQuestion, EngineError> {
        let current = self.question(id)?;
        if caller.is_some_and(|chat| chat != current.chat_id) {
            return Err(EngineError::Other("that question isn't this chat's".into()));
        }
        self.resolve(id, QuestionStatus::Cancelled, None)
    }

    fn resolve(
        &self,
        id: &str,
        status: QuestionStatus,
        answer: Option<&str>,
    ) -> Result<AgentQuestion, EngineError> {
        let (question, changed) = self
            .inner
            .workspace
            .resolve_question(id, status, answer)
            .ok_or_else(|| EngineError::Other(format!("no question {id}")))?;
        if !changed {
            return Err(EngineError::Other(format!(
                "question {id} is already {}",
                status_word(question.status)
            )));
        }
        if !question.blocking {
            return self.deliver_resolution(question);
        }
        Ok(question)
    }

    /// A blocking `ask_question` stopped waiting: answers are delivered as
    /// messages from now on, and with `deliver`, one that already arrived
    /// is delivered now (its tool call ended without it).
    pub fn release(
        &self,
        id: &str,
        caller: Option<&str>,
        deliver: bool,
    ) -> Result<AgentQuestion, EngineError> {
        let current = self.question(id)?;
        if caller.is_some_and(|chat| chat != current.chat_id) {
            return Err(EngineError::Other("that question isn't this chat's".into()));
        }
        if current.blocking {
            self.inner.workspace.set_question_blocking(id, false);
        }
        let current = self.question(id)?;
        if deliver
            && current.delivered_at.is_none()
            && matches!(
                current.status,
                QuestionStatus::Answered | QuestionStatus::Dismissed
            )
        {
            return self.deliver_resolution(current);
        }
        Ok(current)
    }

    fn deliver_resolution(&self, question: AgentQuestion) -> Result<AgentQuestion, EngineError> {
        let text = match question.status {
            QuestionStatus::Answered => {
                questions::answer_message(&question, question.answer.as_deref().unwrap_or_default())
            }
            QuestionStatus::Dismissed => questions::dismissed_message(&question),
            QuestionStatus::Pending | QuestionStatus::Cancelled => return Ok(question),
        };
        self.deliver(&question.chat_id, text)?;
        self.inner.workspace.set_question_delivered(&question.id);
        Ok(self
            .inner
            .workspace
            .question(&question.id)
            .unwrap_or(question))
    }

    /// Put `text` in front of `chat_id`'s agent: steered into a running
    /// turn, or a new turn when it is idle — what the composer's send does.
    fn deliver(&self, chat_id: &str, text: String) -> Result<String, EngineError> {
        self.inner.doc_host.queue_command(
            chat_id,
            SessionCommandPayload::Steer {
                prompt: text,
                message_id: Some(new_id()),
            },
        )
    }

    // ── turn watches ────────────────────────────────────────────────────────

    /// Tell `watcher` when `target`'s turn ends. One watch per pair: a newer
    /// one (a newer send's baseline) replaces the older.
    pub fn notify_turn_end(
        &self,
        watcher: &str,
        target: &str,
        baseline: Option<Session>,
        expect_turn: bool,
        start_message_ids: Vec<String>,
    ) -> Result<String, EngineError> {
        if watcher == target {
            return Err(EngineError::Other("a chat can't watch its own turn".into()));
        }
        if self.inner.workspace.chat(target)?.is_none() {
            return Err(EngineError::Other(format!("no chat {target}")));
        }
        let watch = TurnWatch {
            id: format!("w-{}", new_id()),
            watcher_chat_id: watcher.to_owned(),
            target_chat_id: target.to_owned(),
            progress: TurnProgress::new(baseline, expect_turn),
            start_message_ids,
            created_at_ms: now_ms(),
            settled: None,
        };
        let id = watch.id.clone();
        {
            let mut watches = lock(&self.inner.watches);
            watches.retain(|w| {
                !(w.watcher_chat_id == watcher && w.target_chat_id == target && w.settled.is_none())
            });
            watches.push(watch);
        }
        self.inner.persist();
        self.start();
        self.inner.wake.notify_one();
        Ok(id)
    }

    /// The watches still waiting (tests, diagnostics).
    pub fn turn_watches(&self) -> Vec<TurnWatch> {
        lock(&self.inner.watches).clone()
    }

    fn start(&self) {
        let weak = Arc::downgrade(&self.inner);
        self.inner.started.call_once(|| {
            tokio::spawn(watch_task(weak));
        });
    }
}

fn status_word(status: QuestionStatus) -> &'static str {
    match status {
        QuestionStatus::Pending => "open",
        QuestionStatus::Answered => "answered",
        QuestionStatus::Dismissed => "dismissed",
        QuestionStatus::Cancelled => "cancelled",
    }
}

impl Inner {
    fn persist(&self) {
        let Some(path) = &self.path else { return };
        let watches = lock(&self.watches).clone();
        let result = serde_json::to_vec_pretty(&watches)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                let tmp = path.with_extension("json.tmp");
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(&tmp, path)
            });
        if let Err(error) = result {
            tracing::warn!(%error, "turn watches: save failed");
        }
    }

    /// Advance every watch on the current rows; returns the ones that settled.
    fn evaluate(&self, sessions: &[Session]) -> Vec<(TurnWatch, Option<Chat>)> {
        let now = chrono::Utc::now();
        let mut settled = Vec::new();
        let mut changed = false;
        {
            let mut watches = lock(&self.watches);
            let mut delivering = lock(&self.delivering);
            for watch in watches.iter_mut() {
                if delivering.contains(&watch.id) {
                    continue;
                }
                let chat = self.workspace.chat(&watch.target_chat_id).ok().flatten();
                if watch.settled.is_none() {
                    match &chat {
                        Some(chat) => {
                            let before = watch.progress.saw_working;
                            let session = session_for(sessions, chat);
                            watch.settled = watch.progress.observe(session.as_ref(), now);
                            changed |= before != watch.progress.saw_working;
                        }
                        // Deleted under the watch: say so rather than wait.
                        None => watch.settled = Some(TurnOutcome::Errored),
                    }
                    if watch.settled.is_none() && now_ms() - watch.created_at_ms > WATCH_MAX_AGE_MS
                    {
                        watch.settled = Some(TurnOutcome::TimedOut);
                    }
                    changed |= watch.settled.is_some();
                }
                if watch.settled.is_some() {
                    delivering.insert(watch.id.clone());
                    settled.push((watch.clone(), chat));
                }
            }
        }
        if changed {
            self.persist();
        }
        settled
    }

    /// Notify the watcher, then forget the watch.
    async fn settle(self: Arc<Self>, watch: TurnWatch, chat: Option<Chat>) {
        let outcome = watch.settled.unwrap_or(TurnOutcome::Completed);
        let text = self.notification(&watch, chat.as_ref(), outcome).await;
        let label = chat_label(chat.as_ref(), &watch.target_chat_id);
        let text = zeron_proto::agent_messages::agent_message(&label, &watch.target_chat_id, &text);
        let result = self.doc_host.queue_command(
            &watch.watcher_chat_id,
            SessionCommandPayload::Steer {
                prompt: text,
                message_id: Some(new_id()),
            },
        );
        if let Err(error) = &result {
            tracing::warn!(watch = %watch.id, %error, "turn watch: delivery failed");
        }
        lock(&self.watches).retain(|w| w.id != watch.id);
        lock(&self.delivering).remove(&watch.id);
        self.persist();
    }

    async fn notification(
        &self,
        watch: &TurnWatch,
        chat: Option<&Chat>,
        outcome: TurnOutcome,
    ) -> String {
        let Some(chat) = chat else {
            return "This chat no longer exists, so its turn won't finish.".into();
        };
        let title = chat_label(Some(chat), &chat.id);
        if outcome == TurnOutcome::TimedOut {
            return format!(
                "Chat «{title}» is still working after 24 hours, so Zeron stopped watching it for you. Call wait_for_turn on it again if you still need its result."
            );
        }
        let (replies, pending) = self.reply(watch, chat, outcome).await;
        let mut text = match outcome {
            TurnOutcome::Completed => format!("Chat «{title}» finished its turn."),
            TurnOutcome::AwaitingInput => format!(
                "Chat «{title}» is waiting for an answer (answer it with respond_to_input)."
            ),
            TurnOutcome::Errored => format!("Chat «{title}» stopped with an error."),
            TurnOutcome::Interrupted => format!("Chat «{title}»'s turn was interrupted."),
            TurnOutcome::TimedOut => unreachable!(),
        };
        if let Some(pending) = pending {
            text.push_str(&format!("\n\nIts question: {pending}"));
        }
        match replies {
            Some(reply) if !reply.trim().is_empty() => {
                text.push_str("\n\nIts reply:\n\n");
                text.push_str(&excerpt(&reply));
            }
            _ => {
                if let Some(preview) = chat
                    .last_message_preview
                    .as_deref()
                    .filter(|p| !p.trim().is_empty())
                {
                    text.push_str(&format!("\n\nLast message: {preview}"));
                }
            }
        }
        text.push_str(&format!(
            "\n\n(read_chat {} for the whole transcript.)",
            chat.id
        ));
        text
    }

    /// The awaited turn's reply (assistant messages after the watch began),
    /// waiting a little for a transcript that trails its session row; and
    /// the question it is blocked on, if any.
    async fn reply(
        &self,
        watch: &TurnWatch,
        chat: &Chat,
        outcome: TurnOutcome,
    ) -> (Option<String>, Option<String>) {
        let deadline = tokio::time::Instant::now() + REPLY_GRACE;
        loop {
            let entries = self
                .doc_host
                .open(&chat.id)
                .ok()
                .and_then(|handle| handle.doc().read_entries().ok())
                .unwrap_or_default();
            let rendered = render_entries(&entries, RenderOptions::default());
            let pending = rendered
                .iter()
                .rev()
                .find_map(|m| m.pending_input.clone())
                .map(|p| p.to_string());
            let replies: Vec<_> = rendered
                .iter()
                .filter(|m| m.role == zeron_doc::MessageRole::Assistant)
                .filter(|m| !watch.start_message_ids.contains(&m.id))
                .collect();
            let done = replies
                .iter()
                .any(|m| m.status != Some(zeron_doc::MessageStatus::Streaming));
            let needs_reply = watch.progress.expect_turn && outcome == TurnOutcome::Completed;
            if done || !needs_reply || tokio::time::Instant::now() >= deadline {
                let text = replies
                    .iter()
                    .map(|m| m.text.trim())
                    .filter(|t| !t.is_empty())
                    .next_back()
                    .map(str::to_owned);
                let pending = (outcome == TurnOutcome::AwaitingInput)
                    .then_some(pending)
                    .flatten();
                return (text, pending);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

fn chat_label(chat: Option<&Chat>, id: &str) -> String {
    let short: String = id.chars().take(8).collect();
    match chat
        .and_then(|c| c.title.as_deref())
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        Some(title) => format!("{title} ({short})"),
        None => short,
    }
}

fn excerpt(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= EXCERPT_CHARS {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(EXCERPT_CHARS).collect();
    out.push_str(" …");
    out
}

async fn watch_task(weak: std::sync::Weak<Inner>) {
    let Some(inner) = weak.upgrade() else { return };
    let mut rows = inner
        .workspace
        .merged_sessions_watch(inner.sessions.watch_sessions());
    let notify = inner.wake.clone();
    drop(inner);
    let mut sweep = tokio::time::interval(SWEEP);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let sessions = rows.borrow_and_update().clone();
        for (watch, chat) in inner.evaluate(&sessions) {
            tokio::spawn(inner.clone().settle(watch, chat));
        }
        drop(inner);
        let wake = notify.notified();
        tokio::pin!(wake);
        tokio::select! {
            changed = rows.changed() => if changed.is_err() { return },
            _ = &mut wake => {}
            _ = sweep.tick() => {}
        }
    }
}
