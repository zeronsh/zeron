//! Handing side-chat notices to the agent of the chat that started them.
//!
//! A side chat's host leaves a notice on its parent's doc when a delegated
//! turn ends (see `side_chat_reports`). The parent's host delivers what has
//! piled up, never into a live turn:
//!
//! - at the head of the next prompt anyone sends the parent (a typed message,
//!   a queued row, another chat's send), so one turn answers both;
//! - otherwise, once the agent is free and the queue is empty, as a turn of
//!   its own. A short settle window lets side chats finishing together arrive
//!   as one turn rather than one each.
//!
//! Progress notes ride along but never start a turn. A Stop holds the
//! updates with the queue: the user's next send carries them.

use zeron_doc::ChatNotice;
use zeron_proto::side_chat_updates::{SideChatUpdate, updates_prompt, with_updates};

use super::*;

/// How long the newest notice settles before a turn of its own starts, so
/// side chats finishing within moments of each other share it.
pub(super) const NOTICE_SETTLE_MS: i64 = 1_500;

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

    /// Report this engine's side chats to their parents
    /// ([`crate::side_chat_reports`]).
    pub fn spawn_side_chat_reports(
        &self,
        sessions: watch::Receiver<Vec<zeron_proto::Session>>,
        chats: watch::Receiver<Vec<zeron_proto::Chat>>,
    ) {
        self.spawn_worker(crate::side_chat_reports::run(self.clone(), sessions, chats));
    }

    /// Run a side-chat report's catch-up as one of this host's workers.
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

    /// Host-only: start a turn for the notices waiting on `handle`'s chat,
    /// when nothing else will carry them and the agent is free.
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
        let _drain = handle.drain_lock.lock().await;
        if handle.queue_paused.load(Ordering::Acquire) {
            return;
        }
        let pending = pending_notices(handle);
        if !pending.iter().any(|notice| notice.kind.wakes()) {
            return;
        }
        // The turn in flight, the next queued row, or a prompt about to run
        // carries them instead.
        if sessions.turn_in_flight(&handle.chat_id)
            || handle.doc.read_queue().is_ok_and(|queue| !queue.is_empty())
            || self.prompt_waiting(handle)
        {
            return;
        }
        let newest = pending.iter().map(|n| n.issued_at).max().unwrap_or(0);
        let due = newest.min(now_ms()) + NOTICE_SETTLE_MS;
        if now_ms() < due {
            self.arm_notice_settle(handle, due);
            return;
        }
        let prompt = updates_prompt(
            &updates(&pending),
            &self.running_side_chats(&handle.chat_id),
            false,
        );
        let ids: Vec<String> = pending.iter().map(|n| n.id.clone()).collect();
        if let Err(err) = self.deliver_notices(handle, &ids) {
            tracing::warn!(chat = %handle.chat_id, error = %err, "notice delivery stamp failed");
            return;
        }
        let request = self.next_turn_request(&sessions, &handle.chat_id, &prompt);
        let Some(mut request) = request else {
            tracing::warn!(chat = %handle.chat_id, "side-chat updates have no run config to start a turn");
            self.return_notices(handle, &ids);
            return;
        };
        request.prompt = prompt;
        request.resume = None;
        request.attachments = Vec::new();
        let harness = self.harness_for_request(&handle.chat_id, &request);
        tracing::info!(chat = %handle.chat_id, count = ids.len(), "side-chat updates start a turn");
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
            tracing::warn!(chat = %handle.chat_id, error = %err, "side-chat updates turn failed to start");
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
        *prompt = with_updates(
            &updates(&pending),
            &self.running_side_chats(&handle.chat_id),
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

    /// The side chats of `chat_id` still working, as (id, title).
    fn running_side_chats(&self, chat_id: &str) -> Vec<(String, String)> {
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
                let title = chat
                    .title
                    .clone()
                    .unwrap_or_else(|| "Untitled side chat".into());
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

fn updates(notices: &[ChatNotice]) -> Vec<SideChatUpdate> {
    notices
        .iter()
        .map(|notice| SideChatUpdate {
            kind: notice.kind,
            chat_id: notice.chat_id.clone(),
            title: notice.title.clone(),
            text: notice.text.clone(),
        })
        .collect()
}

/// The turn a side chat just ended, read off its transcript for its parent.
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
    /// The turn's prompt was side-chat updates and nothing the user wrote.
    pub updates_only: bool,
}

impl EndedTurn {
    /// A turn that only took in side-chat updates and had nothing for the
    /// user: no reply, or the quiet one.
    pub(crate) fn quiet(&self) -> bool {
        self.updates_only
            && (self.reply.trim().is_empty()
                || zeron_proto::side_chat_updates::is_quiet_reply(&self.reply))
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
    let updates_only = zeron_proto::side_chat_updates::split_updates(&user_text)
        .is_some_and(|(_, typed)| typed.trim().is_empty());
    let mut turn = EndedTurn {
        delegator,
        updates_only,
        turn_key: user.id.clone(),
        ..EndedTurn::default()
    };
    let mut reply: Option<&str> = None;
    for entry in &entries[start + 1..] {
        if entry.role != MessageRole::Assistant {
            continue;
        }
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
