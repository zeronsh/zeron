//! Each host keeps one voice orchestrator chat: every call on this device
//! resumes it, so the orchestrator remembers earlier calls. Hosts never share
//! it — another computer has its own. Once the synced transcript passes
//! [`ROTATE_AFTER`] entries, the next call moves to a fresh chat that inherits
//! the Codex thread: memory continues, the transcript starts small again. The
//! old segment is removed only once its successor holds a message.
use super::*;
use zeron_proto::voice::{ORCHESTRATOR_CHAT_PREFIX, ORCHESTRATOR_CHAT_TITLE, VoiceRejection};
use zeron_proto::{Chat, SessionStatus};

/// Transcript entries after which the next call starts a new segment.
pub(super) const ROTATE_AFTER: usize = 1000;

/// This device's current orchestrator chat: its newest one, if any.
fn current<'a>(chats: &'a [Chat], device: &str) -> Option<&'a Chat> {
    chats
        .iter()
        .filter(|c| {
            zeron_proto::voice::is_orchestrator_chat(&c.id) && c.device_id == device && !c.archived
        })
        .max_by_key(|c| c.created_at)
}

impl EngineRpc {
    /// The chat a call on this device runs in: reused, rotated or created.
    /// Callers hold the voice preparation lock and have checked that no call
    /// is live, so nothing is rotated from under one.
    pub(super) async fn voice_chat(&self, config: ChatConfig) -> Result<String, VoiceRejection> {
        let device = self.engine_info.device_id.clone();
        let chats = self
            .workspace
            .read_chats()
            .map_err(|_| VoiceRejection::Protocol)?;
        let chat = current(&chats, &device).cloned();
        let Some(chat) = chat else {
            return self.new_voice_chat(config, None);
        };
        // A question left over from an ended call would block this one.
        if self
            .sessions
            .session_status(&chat.id)
            .is_some_and(|s| s.status == SessionStatus::AwaitingInput)
        {
            let _ = self.sessions.interrupt(&chat.id).await;
        }
        // Delegated work still running keeps its chat and configuration.
        if self.sessions.turn_in_flight(&chat.id) {
            return Ok(chat.id);
        }
        let entries = self
            .doc_host
            .open(&chat.id)
            .map_err(|_| VoiceRejection::Protocol)?
            .doc()
            .read_entries()
            .map_err(|_| VoiceRejection::Protocol)?
            .len();
        if entries > 0 {
            self.prune_superseded(&chats, &chat).await;
        }
        if entries < ROTATE_AFTER {
            if chat.config.as_ref() != Some(&config) {
                // A warm process cannot take another configuration.
                let _ = self.sessions.interrupt(&chat.id).await;
                self.workspace
                    .set_chat_config(&chat.id, &config)
                    .map_err(|_| VoiceRejection::Protocol)?;
            }
            return Ok(chat.id);
        }
        // The old segment's warm process holds the Codex thread; stop it before
        // the next segment resumes the same thread.
        let _ = self.sessions.interrupt(&chat.id).await;
        let thread = self
            .workspace
            .chat_harness_session(&chat.id)
            .filter(|(id, _)| !id.is_empty());
        // The old segment stays until the new one holds a message: a failed
        // start may remove the empty successor, and then the next call
        // rotates again from here instead of losing the thread.
        self.new_voice_chat(config, thread)
    }

    /// Removes this device's earlier segments of `current`'s Codex thread.
    /// Orchestrators from other threads (calls before segments) are kept.
    async fn prune_superseded(&self, chats: &[Chat], current: &Chat) {
        let Some(thread) = current
            .harness_session_id
            .as_deref()
            .filter(|t| !t.is_empty())
        else {
            return;
        };
        for old in chats.iter().filter(|c| {
            c.id != current.id
                && c.device_id == current.device_id
                && zeron_proto::voice::is_orchestrator_chat(&c.id)
                && c.harness_session_id.as_deref() == Some(thread)
        }) {
            if self.sessions.turn_in_flight(&old.id) {
                continue;
            }
            let _ = self.sessions.interrupt(&old.id).await;
            let _ = self.workspace.delete_chat(&old.id);
            self.doc_host.purge_chat(&old.id);
        }
    }

    fn new_voice_chat(
        &self,
        config: ChatConfig,
        thread: Option<(String, Option<String>)>,
    ) -> Result<String, VoiceRejection> {
        let chat = format!("{ORCHESTRATOR_CHAT_PREFIX}{}", uuid::Uuid::new_v4());
        // No space: a projectless chat whose cwd is the host's `~`.
        self.workspace
            .create_chat(
                &chat,
                None,
                Some(&self.engine_info.device_id),
                Some(config),
                None,
            )
            .map_err(|_| VoiceRejection::Protocol)?;
        // Same title on every host: the transcript is reachable from every
        // client's voice controls, never an untitled "New session".
        self.workspace
            .rename_chat(&chat, ORCHESTRATOR_CHAT_TITLE)
            .map_err(|_| VoiceRejection::Protocol)?;
        if let Some((id, cwd)) = thread {
            self.workspace
                .set_chat_harness_session(&chat, &id, cwd.as_deref().unwrap_or(""));
        }
        Ok(chat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(model: &str) -> ChatConfig {
        serde_json::from_value(serde_json::json!({
            "harness": "codex", "model": model, "sandbox": "workspace-write"
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn calls_reuse_the_hosts_chat_and_rotate_a_long_transcript_keeping_the_thread() {
        let temp = tempfile::tempdir().unwrap();
        let core = crate::EngineCore::assemble_with_profile(
            crate::EngineProfile::local(temp.path()).unwrap(),
            std::sync::Arc::new(HarnessRegistry::new()),
            HarnessId::Codex,
            None,
        )
        .unwrap();
        let rpc = core.rpc_service();
        // Another host's orchestrator is never this device's.
        let foreign = format!("{ORCHESTRATOR_CHAT_PREFIX}foreign");
        core.workspace
            .create_chat(&foreign, None, Some("other-device"), None, None)
            .unwrap();

        let first = rpc.voice_chat(config("a")).await.unwrap();
        assert_ne!(first, foreign);
        let row = core.workspace.chat(&first).unwrap().unwrap();
        assert_eq!(row.title.as_deref(), Some(ORCHESTRATOR_CHAT_TITLE));
        assert_eq!(row.device_id, core.device_id);

        // The next call resumes it, taking the configuration it asks for.
        assert_eq!(rpc.voice_chat(config("b")).await.unwrap(), first);
        let row = core.workspace.chat(&first).unwrap().unwrap();
        assert_eq!(row.config, Some(config("b")));

        core.workspace
            .set_chat_harness_session(&first, "codex-thread", "/home/me");
        let doc = core.doc_host.open(&first).unwrap();
        for i in 0..ROTATE_AFTER {
            doc.write_user_message(&format!("m{i}"), "hi", i as i64)
                .unwrap();
        }
        let inherited = Some(("codex-thread".into(), Some("/home/me".into())));
        let next = rpc.voice_chat(config("b")).await.unwrap();
        assert_ne!(next, first);
        assert_eq!(core.workspace.chat_harness_session(&next), inherited);

        // A failed start removes its empty successor; the old segment is
        // still there, so the next call rotates again on the same thread.
        core.workspace.delete_chat(&next).unwrap();
        core.doc_host.purge_chat(&next);
        let retry = rpc.voice_chat(config("b")).await.unwrap();
        assert!(retry != first && retry != next);
        assert_eq!(core.workspace.chat_harness_session(&retry), inherited);

        // An empty successor keeps its predecessor; one with a message
        // supersedes it. Other orchestrators and threads are never touched.
        assert_eq!(rpc.voice_chat(config("b")).await.unwrap(), retry);
        assert!(core.workspace.chat(&first).unwrap().is_some());
        core.doc_host
            .open(&retry)
            .unwrap()
            .write_user_message("hello", "hi", 0)
            .unwrap();
        assert_eq!(rpc.voice_chat(config("b")).await.unwrap(), retry);
        assert!(core.workspace.chat(&first).unwrap().is_none());
        assert!(core.workspace.chat(&foreign).unwrap().is_some());
        core.sessions.shutdown().await;
    }
}
