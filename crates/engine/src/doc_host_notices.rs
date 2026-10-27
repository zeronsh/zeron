//! Handing chat events to the agent of the chat that started the chats
//! reporting (see `zeron_proto::chat_events` and `chat_reports`).
//!
//! A chat its user talks to hears them only between turns, never in a live
//! one (an event must not steer an agent off what it is telling its user):
//!
//! - at the head of the next prompt anyone sends it (a typed message, a
//!   queued row, another chat's send), so one turn answers both;
//! - otherwise, once the agent is free and the queue is empty, as a turn of
//!   its own. A short settle window lets chats finishing together arrive as
//!   one turn rather than one each.
//!
//! A task's coordinator supervises its agents, so it hears them as they go:
//! steered into its live turn at the next step, or as a turn of its own.
//! A Stop holds the events with the queue: the next send carries them.

use zeron_doc::ChatNotice;
use zeron_proto::chat_events::{Audience, ChatEvent, events_prompt, with_events};

use super::*;

/// How long the newest notice settles before it is delivered, so chats
/// finishing within moments of each other share a turn.
pub(super) const NOTICE_SETTLE_MS: i64 = 3_000;

impl DocHost {
    /// Leave `notice` on `chat_id`'s doc for its agent. `false` when it was
    /// already there (or already acknowledged).
    pub fn post_notice(&self, chat_id: &str, notice: &ChatNotice) -> Result<bool, EngineError> {
        let handle = self.open(chat_id)?;
        let fresh = handle.doc.push_notice(notice)?;
        if fresh {
            handle.publish_notices();
            self.nudge_remote_host(chat_id);
        }
        Ok(fresh)
    }

    /// Leave chat `notice.chat_id`'s report on `parent`'s doc, remembering
    /// `turn_key` as its last reported turn. `false` when it was already
    /// there.
    pub(crate) fn post_report(
        &self,
        parent: &str,
        notice: &ChatNotice,
        turn_key: Option<&str>,
    ) -> Result<bool, EngineError> {
        let handle = self.open(parent)?;
        let fresh = handle.doc.push_notice(notice)?;
        if let Some(key) = turn_key {
            handle.doc.set_reported(&notice.chat_id, key)?;
        }
        if fresh {
            handle.publish_notices();
            self.nudge_remote_host(parent);
        }
        Ok(fresh)
    }

    /// This engine hosts `chat_id`.
    pub(crate) fn hosts(&self, chat_id: &str) -> bool {
        self.is_host(chat_id)
    }

    /// The last turn of chat `child` reported on `parent`'s doc.
    pub(crate) fn last_reported(&self, parent: &str, child: &str) -> Option<String> {
        self.open(parent).ok()?.doc.last_reported(child)
    }

    /// Coordinator `child` reported to `parent` at or after `since` (epoch
    /// millis).
    pub(crate) fn reported_since(&self, parent: &str, child: &str, since: i64) -> bool {
        self.open(parent)
            .ok()
            .and_then(|handle| handle.doc.read_notices().ok())
            .is_some_and(|notices| {
                notices.iter().any(|notice| {
                    notice.chat_id == child && notice.kind.is_task_report() && notice.issued_at >= since
                })
            })
    }

    /// Some agent coordinator `chat_id` started is working or waiting on an
    /// answer.
    pub(crate) fn agents_at_work(&self, chat_id: &str) -> bool {
        let Some(workspace) = self.workspace() else {
            return false;
        };
        let (Ok(chats), Ok(sessions)) = (workspace.read_chats(), workspace.read_sessions()) else {
            return false;
        };
        let now = chrono::Utc::now();
        chats
            .iter()
            .filter(|chat| !chat.archived && chat.parent_chat_id.as_deref() == Some(chat_id))
            .any(|chat| {
                sessions.iter().any(|s| {
                    s.chat_id == chat.id
                        && matches!(
                            s.status,
                            zeron_proto::SessionStatus::Working | zeron_proto::SessionStatus::AwaitingInput
                        )
                        && !zeron_proto::view::session_stale(s, now)
                })
            })
    }

    /// Report the chats started by chats this engine hosts to them
    /// ([`crate::chat_reports`]).
    pub fn spawn_chat_reports(
        &self,
        sessions: watch::Receiver<Vec<zeron_proto::Session>>,
        chats: watch::Receiver<Vec<zeron_proto::Chat>>,
    ) {
        self.spawn_worker(crate::chat_reports::run(self.clone(), sessions, chats));
    }

    /// Run a report's catch-up as one of this host's workers.
    pub(crate) fn spawn_report(&self, fut: impl std::future::Future<Output = ()> + Send + 'static) {
        self.spawn_worker(fut);
    }

    /// Mark `ids` on `chat_id`'s doc delivered: its agent already read them
    /// (a `wait_for_turn` returned the reply), so no turn should repeat them.
    pub fn ack_notices(&self, chat_id: &str, ids: &[String]) -> Result<(), EngineError> {
        if ids.is_empty() {
            return Ok(());
        }
        let handle = self.open(chat_id)?;
        handle.doc.mark_notices_delivered(ids, now_ms())?;
        handle.publish_notices();
        Ok(())
    }

    /// Host-only: hand the notices waiting on `handle`'s chat to its agent,
    /// when nothing else will carry them: as a turn of its own once the agent
    /// is free, or, for a coordinator, into its live turn.
    pub(super) async fn drain_notices(&self, handle: &Arc<ChatDocHandle>) {
        let Some(_dispatch) = self.dispatch_guard() else {
            return;
        };
        let Some(sessions) = self.sessions() else {
            return;
        };
        if !self.is_host(&handle.chat_id) {
            return;
        }
        let drain = handle.drain_lock.lock().await;
        if handle.queue_paused.load(Ordering::Acquire) {
            return;
        }
        let pending = pending_notices(handle);
        if pending.is_empty() {
            return;
        }
        let coordinator = self.is_coordinator(&handle.chat_id);
        // The next queued row, or a prompt about to run, carries them; for a
        // chat its user talks to, so does the turn in flight, at its end.
        if handle.doc.read_queue().is_ok_and(|queue| !queue.is_empty())
            || self.prompt_waiting(handle)
            || (!coordinator && sessions.turn_in_flight(&handle.chat_id))
        {
            return;
        }
        let newest = pending.iter().map(|n| n.issued_at).max().unwrap_or(0);
        let due = newest.min(now_ms()) + NOTICE_SETTLE_MS;
        if now_ms() < due {
            self.arm_notice_settle(handle, due);
            return;
        }
        let audience = if coordinator {
            Audience::Coordinator
        } else {
            Audience::Main {
                user_follows: false,
            }
        };
        let prompt = events_prompt(
            &events(&pending),
            &self.running_children(&handle.chat_id),
            audience,
        );
        if coordinator {
            // Into the live turn at its next step, or as a new turn; never
            // interrupting one. The drain lock goes first: delivery may
            // re-enter the queue drain.
            let ids: Vec<String> = pending.iter().map(|n| n.id.clone()).collect();
            if let Err(err) = self.deliver_notices(handle, &ids) {
                tracing::warn!(chat = %handle.chat_id, error = %err, "notice delivery stamp failed");
                return;
            }
            drop(drain);
            tracing::info!(chat = %handle.chat_id, count = ids.len(), "agents' events reach their coordinator");
            // Boxed: delivery may drain the queue, which drains notices.
            if let Err(err) =
                Box::pin(self.deliver_prompt(&sessions, handle, &prompt, Some(new_id()), now_ms()))
                    .await
            {
                tracing::warn!(chat = %handle.chat_id, error = %err, "agents' events failed to reach their coordinator");
                self.return_notices(handle, &ids);
            }
            return;
        }
        let ids: Vec<String> = pending.iter().map(|n| n.id.clone()).collect();
        if let Err(err) = self.deliver_notices(handle, &ids) {
            tracing::warn!(chat = %handle.chat_id, error = %err, "notice delivery stamp failed");
            return;
        }
        let request = self.next_turn_request(&sessions, &handle.chat_id, &prompt);
        let Some(mut request) = request else {
            tracing::warn!(chat = %handle.chat_id, "chat events have no run config to start a turn");
            self.return_notices(handle, &ids);
            return;
        };
        request.prompt = prompt;
        request.resume = None;
        request.attachments = Vec::new();
        let harness = self.harness_for_request(&handle.chat_id, &request);
        tracing::info!(chat = %handle.chat_id, count = ids.len(), "chat events start a turn");
        if let Err(err) = self
            .dispatch_with_source_context(
                &sessions,
                &handle.chat_id,
                harness,
                request,
                Some(new_id()),
            )
            .await
        {
            tracing::warn!(chat = %handle.chat_id, error = %err, "chat events turn failed to start");
            // Like a queued send that failed: hold, and let the user's next
            // send carry them rather than retrying on every status change.
            handle.queue_paused.store(true, Ordering::Release);
            self.return_notices(handle, &ids);
        }
    }

    /// Put the waiting notices at the head of `prompt`, stamping them
    /// delivered. The ids come back so a send that never starts can return
    /// them ([`Self::return_notices`]).
    pub(super) fn carry_notices(
        &self,
        handle: &Arc<ChatDocHandle>,
        prompt: &mut String,
    ) -> Vec<String> {
        let pending = pending_notices(handle);
        if pending.is_empty() {
            return Vec::new();
        }
        let ids: Vec<String> = pending.iter().map(|n| n.id.clone()).collect();
        if let Err(err) = self.deliver_notices(handle, &ids) {
            tracing::warn!(chat = %handle.chat_id, error = %err, "notice delivery stamp failed");
            return Vec::new();
        }
        *prompt = with_events(
            &events(&pending),
            &self.running_children(&handle.chat_id),
            prompt,
        );
        ids
    }

    /// The send that was to carry `ids` never started: they wait again.
    pub(super) fn return_notices(&self, handle: &Arc<ChatDocHandle>, ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        if let Err(err) = handle.doc.mark_notices_pending(ids) {
            tracing::warn!(chat = %handle.chat_id, error = %err, "notice return failed");
        }
        handle.publish_notices();
    }

    fn deliver_notices(
        &self,
        handle: &Arc<ChatDocHandle>,
        ids: &[String],
    ) -> Result<(), EngineError> {
        handle.doc.mark_notices_delivered(ids, now_ms())?;
        handle
            .doc
            .prune_notices(zeron_doc::DELIVERED_NOTICES_KEPT)?;
        handle.publish_notices();
        Ok(())
    }

    /// A Run or Steer command not executed yet: it will carry the notices.
    fn prompt_waiting(&self, handle: &Arc<ChatDocHandle>) -> bool {
        let is_processed = |id: &str| self.inner.store.is_processed(id).unwrap_or(false);
        handle.doc.read_commands().is_ok_and(|commands| {
            commands.iter().any(|c| {
                c.status == SessionCommandStatus::Pending
                    && matches!(
                        c.payload,
                        SessionCommandPayload::Run { .. } | SessionCommandPayload::Steer { .. }
                    )
                    && !is_processed(&c.id)
            })
        })
    }

    /// Whether `chat_id` coordinates a task (it hears its agents mid-turn).
    fn is_coordinator(&self, chat_id: &str) -> bool {
        self.workspace()
            .and_then(|ws| ws.chat(chat_id).ok().flatten())
            .is_some_and(|chat| chat.is_coordinator())
    }

    /// The chats `chat_id` started that are still working, as (id, title).
    fn running_children(&self, chat_id: &str) -> Vec<(String, String)> {
        let Some(workspace) = self.workspace() else {
            return Vec::new();
        };
        let (Ok(chats), Ok(sessions)) = (workspace.read_chats(), workspace.read_sessions()) else {
            return Vec::new();
        };
        let now = chrono::Utc::now();
        chats
            .into_iter()
            .filter(|chat| !chat.archived && chat.parent_chat_id.as_deref() == Some(chat_id))
            .filter(|chat| {
                sessions.iter().any(|s| {
                    s.chat_id == chat.id
                        && s.status == zeron_proto::SessionStatus::Working
                        && !zeron_proto::view::session_stale(s, now)
                })
            })
            .map(|chat| {
                let title = chat.title.clone().unwrap_or_else(|| "Untitled chat".into());
                (chat.id, title)
            })
            .collect()
    }

    /// Re-drain once the newest notice has settled. One timer per chat.
    fn arm_notice_settle(&self, handle: &Arc<ChatDocHandle>, due: i64) {
        if handle.notice_settle_armed.swap(true, Ordering::AcqRel) {
            return;
        }
        let weak = Arc::downgrade(handle);
        let host = self.clone();
        self.spawn_worker(async move {
            let wait = (due - now_ms()).clamp(0, NOTICE_SETTLE_MS * 4) as u64;
            tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
            let Some(handle) = weak.upgrade() else { return };
            handle.notice_settle_armed.store(false, Ordering::Release);
            host.drain_queue(&handle).await;
        });
    }
}

fn pending_notices(handle: &ChatDocHandle) -> Vec<ChatNotice> {
    handle
        .doc
        .read_notices()
        .unwrap_or_default()
        .into_iter()
        .filter(ChatNotice::pending)
        .collect()
}

fn events(notices: &[ChatNotice]) -> Vec<ChatEvent> {
    notices
        .iter()
        .map(|notice| ChatEvent {
            kind: notice.kind,
            chat_id: notice.chat_id.clone(),
            title: notice.title.clone(),
            text: notice.text.clone(),
        })
        .collect()
}

/// The turn a chat just ended, read off its transcript for its parent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EndedTurn {
    /// The chat whose message started (or last steered) the turn, when another
    /// chat sent it: the parent reports only on work it delegated.
    pub delegator: Option<String>,
    /// The id of that message: one turn, one notice.
    pub turn_key: String,
    /// The turn's final reply: its last prose, after whatever it narrated
    /// on the way.
    pub reply: String,
    /// The turn's last error, when it failed.
    pub error: Option<String>,
    /// The question it stopped on: request id, and the question as text.
    pub pending_input: Option<(String, String)>,
    /// The turn's prompt was chat events and nothing anyone wrote.
    pub events_only: bool,
    /// Epoch millis the turn's prompt was written.
    pub started_at: i64,
    /// Its last segment is still streaming: a replica of a chat on another
    /// machine can see the session settle before its reply does.
    pub streaming: bool,
}

impl EndedTurn {
    /// A turn that only took in chat events and had nothing for the user: no
    /// reply, or the quiet one.
    pub(crate) fn quiet(&self) -> bool {
        self.events_only
            && (self.reply.trim().is_empty()
                || zeron_proto::chat_events::is_quiet_reply(&self.reply))
    }
}

impl DocHost {
    /// `chat_id`'s latest turn as its transcript has it: everything after
    /// its last user message. `None` without one.
    pub(crate) fn ended_turn(&self, chat_id: &str) -> Option<EndedTurn> {
        let handle = self.open(chat_id).ok()?;
        let entries = handle.doc.read_entries().ok()?;
        ended_turn(&entries)
    }
}

pub(crate) fn ended_turn(entries: &[SessionMessageEntry]) -> Option<EndedTurn> {
    let start = entries
        .iter()
        .rposition(|entry| entry.role == MessageRole::User)?;
    let user = &entries[start];
    let user_text: String = user
        .parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let delegator = zeron_proto::agent_messages::agent_message_sender(&user_text)
        .map(|(sender, _)| sender.chat_id);
    let events_only = zeron_proto::chat_events::split_events(&user_text)
        .is_some_and(|(_, typed)| typed.trim().is_empty());
    let mut turn = EndedTurn {
        delegator,
        events_only,
        started_at: user.created_at,
        turn_key: user.id.clone(),
        ..EndedTurn::default()
    };
    let mut reply: Option<&str> = None;
    for entry in &entries[start + 1..] {
        if entry.role != MessageRole::Assistant {
            continue;
        }
        turn.streaming = entry.status == Some(zeron_doc::MessageStatus::Streaming);
        for part in &entry.parts {
            match part {
                MessagePart::Text { text, .. } if !text.trim().is_empty() => {
                    reply = Some(text.trim());
                }
                MessagePart::Error { message, .. } => turn.error = Some(message.clone()),
                MessagePart::Input {
                    request_id,
                    questions,
                    resolved: false,
                    ..
                } => {
                    let text = questions
                        .iter()
                        .map(|q| {
                            let mut line = q.question.trim().to_owned();
                            if !q.header.trim().is_empty() {
                                line = format!("{}: {line}", q.header.trim());
                            }
                            if !q.options.is_empty() {
                                line.push_str(&format!(" (options: {})", q.options.join(" / ")));
                            }
                            line
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    turn.pending_input = Some((request_id.clone(), text));
                }
                _ => {}
            }
        }
    }
    turn.reply = reply.unwrap_or_default().to_owned();
    Some(turn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_doc::MessageStatus;

    fn entry(id: &str, role: MessageRole, text: &str, status: Option<MessageStatus>) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role,
            parts: vec![MessagePart::Text {
                id: format!("{id}-text"),
                text: text.into(),
            }],
            created_at: 1,
            device_id: "dev".into(),
            status,
            continuation_of: None,
            duration_ms: None,
        }
    }

    #[test]
    fn a_turn_s_reply_is_its_last_prose_and_waits_out_a_stream() {
        let mut entries = vec![
            entry("u1", MessageRole::User, "Write the tests", None),
            entry("a1", MessageRole::Assistant, "Running them now.", Some(MessageStatus::Complete)),
            entry("a2", MessageRole::Assistant, "All 14 pass", Some(MessageStatus::Streaming)),
        ];
        let turn = ended_turn(&entries).unwrap();
        assert_eq!((turn.turn_key.as_str(), turn.reply.as_str()), ("u1", "All 14 pass"));
        assert!(turn.streaming);
        entries[2].status = Some(MessageStatus::Complete);
        assert!(!ended_turn(&entries).unwrap().streaming);
    }
}
