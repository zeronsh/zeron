//! What dynamic workflows need from the doc host: the service slot, machine
//! messages into the parent chat's queue, transcript markers, and a nudge for
//! the goal controller. The run logic itself lives in `crate::workflow`.

use zeron_proto::MessageOrigin;

use super::*;

impl DocHost {
    /// Wire the workflow runner (engine assembly).
    pub fn set_workflows(&self, workflows: crate::workflow::WorkflowService) {
        *lock(&self.inner.workflows) = Some(workflows);
    }

    pub fn workflows(&self) -> Option<crate::workflow::WorkflowService> {
        lock(&self.inner.workflows).clone()
    }

    /// Queue a machine-origin user message into `chat_id`, delivered through
    /// the normal queue (so ordering and "never interrupt a running turn"
    /// are the queue's own rules). Idempotent by `id`: a message already
    /// queued or already in the transcript is not queued again. `true` when
    /// this call queued it.
    pub(crate) fn enqueue_machine_message(
        &self,
        chat_id: &str,
        id: &str,
        text: &str,
        origin: MessageOrigin,
    ) -> Result<bool, EngineError> {
        let handle = self.open(chat_id)?;
        let queue = handle.doc.read_queue()?;
        let delivered = handle.doc.read_entries()?.iter().any(|e| e.id == id);
        if delivered || queue.iter().any(|row| row.id == id) {
            return Ok(false);
        }
        let mut row = QueuedMessage::new(
            id.to_owned(),
            text.to_owned(),
            self.inner.config.device_id.clone(),
        );
        row.issued_at = now_ms();
        row.origin = Some(origin);
        handle.doc.push_queued(&row)?;
        // A queue frozen by an earlier Stop would hold this forever; thaw it
        // only when no person's row is in it (those are never reached).
        if queue.is_empty() {
            handle.queue_paused.store(false, Ordering::Release);
        }
        handle.publish_queue();
        Ok(true)
    }

    /// Append a system-role marker row (idempotent by `id`).
    pub(crate) fn push_system_marker(
        &self,
        chat_id: &str,
        id: &str,
        text: &str,
        origin: MessageOrigin,
    ) -> Result<(), EngineError> {
        let handle = self.open(chat_id)?;
        if handle.doc.read_entries()?.iter().any(|e| e.id == id) {
            return Ok(());
        }
        handle.doc.push_message(&SessionMessageEntry {
            origin: Some(origin),
            id: id.to_owned(),
            role: MessageRole::System,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: text.to_owned(),
            }],
            created_at: now_ms(),
            device_id: handle.device_id.clone(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        })?;
        Ok(())
    }

    /// The models a harness offers (cached catalog; `None` when it cannot be
    /// listed — callers then trust the caller's pick rather than guess).
    pub(crate) async fn list_harness_models(
        &self,
        harness: zeron_proto::HarnessId,
    ) -> Option<Vec<zeron_proto::Model>> {
        let sessions = self.sessions()?;
        let repos = self.inner.repos.get()?;
        let harness = sessions.resolve_harness(harness).ok()?;
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            crate::model_catalogs::list_with_lease(repos.data_dir(), harness, false, None),
        )
        .await
        .ok()?
        .ok()
    }

    /// Execute a workflow control command from the command plane (host-only,
    /// like every command). The user is the requester, so a resume needs no
    /// further approval.
    pub(super) async fn apply_workflow_command(
        &self,
        handle: &Arc<ChatDocHandle>,
        command: &zeron_proto::WorkflowCommand,
    ) -> Result<(SessionCommandStatus, Option<String>), EngineError> {
        use zeron_proto::WorkflowCommand as C;
        let Some(workflows) = self.workflows() else {
            return Ok((
                SessionCommandStatus::Rejected,
                Some("workflows are not available".into()),
            ));
        };
        let owned = |run_id: &str| {
            workflows
                .list(Some(&handle.chat_id))
                .iter()
                .any(|r| r.run_id == run_id)
        };
        let run_id = match command {
            C::Stop { run_id, .. } | C::Resume { run_id } | C::Answer { run_id, .. } => run_id,
        };
        if !owned(run_id) {
            return Ok((
                SessionCommandStatus::Rejected,
                Some(format!("no such workflow run in this chat: {run_id}")),
            ));
        }
        let outcome = match command {
            C::Stop { run_id, reason } => workflows
                .stop(run_id, reason.as_deref())
                .map(|_| ())
                .map_err(|e| e.to_string()),
            C::Resume { run_id } => {
                // Resuming waits for nothing; run it off the command drain.
                let workflows = workflows.clone();
                let run_id = run_id.clone();
                self.spawn_worker(async move {
                    if let Err(err) = workflows.resume(&run_id, None, true).await {
                        tracing::warn!(run = %run_id, error = %err, "workflow resume failed");
                    }
                });
                Ok(())
            }
            C::Answer {
                run_id,
                qid,
                answer,
            } => workflows
                .resolve_question(run_id, qid, answer)
                .await
                .map_err(|e| e.to_string()),
        };
        Ok(match outcome {
            Ok(()) => (SessionCommandStatus::Applied, None),
            Err(message) => (SessionCommandStatus::Rejected, Some(message)),
        })
    }

    /// Ask the goal controller to look again (a workflow it was waiting on
    /// just settled).
    pub async fn goal_nudge(&self, chat_id: &str) {
        if let Ok(handle) = self.open(chat_id) {
            self.goal_tick(&handle).await;
        }
    }
}
