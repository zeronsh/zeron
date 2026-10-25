//! Experimental transcript editing: rewrite what an agent will *believe* was
//! said in a chat.
//!
//! Zeron's transcript only mirrors a run; the agent resumes from its own
//! stored session. So an edit writes a NEW native session holding the edited
//! history (the harness driver knows the provider's format), points the chat
//! at it and leaves the original untouched: undo is pointing back. The
//! previous session ids are kept per chat in a small host-local file next to
//! the engine's other device state, never in the synced registry.
//!
//! Everything here runs on the chat's host, with nothing running: the
//! provider's runtime is parked on the old session, so an idle one is retired
//! first and the next prompt relaunches it on the new one.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use zeron_doc::{MessageRole, ProviderSessionMark};
use zeron_harness::{Harness, HarnessError};
use zeron_proto::native_history::{
    ApplyNativeEditsParams, NativeEdit, NativeHistory, NativeItemKind, NativeRole,
};
use zeron_proto::{HarnessId, RunRequest, SandboxLevel};

use crate::doc_host::DocHost;
use crate::registry::HarnessRegistry;
use crate::sessions::SessionsEngine;
use crate::workspace_host::WorkspaceHost;

const FILE_NAME: &str = "native-history-undo.json";
/// Earlier versions kept per chat; the oldest go first.
const MAX_UNDO: usize = 20;

const BUSY: &str = "Stop the agent before editing";
const CHANGED: &str = "The conversation changed since you started editing";
const UNREADABLE: &str = "Couldn't read this conversation from the agent";

/// A transcript text the edit changed, so undo can change it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TextRestore {
    /// `user` or `assistant`.
    role: String,
    /// What the transcript says now.
    current: String,
    /// What it said before the edit.
    original: String,
}

/// One step back: the session the chat resumed before an edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UndoStep {
    session_id: String,
    #[serde(default)]
    restore: Vec<TextRestore>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    chats: HashMap<String, Vec<UndoStep>>,
}

/// Per-chat stack of earlier sessions, persisted on this device.
pub(crate) struct UndoStore {
    path: PathBuf,
    stored: Mutex<Stored>,
}

impl UndoStore {
    pub(crate) fn open(data_dir: &Path) -> Self {
        let path = data_dir.join(FILE_NAME);
        let stored = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            stored: Mutex::new(stored),
        }
    }

    pub(crate) fn can_undo(&self, chat_id: &str) -> bool {
        self.stored
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .chats
            .get(chat_id)
            .is_some_and(|stack| !stack.is_empty())
    }

    fn peek(&self, chat_id: &str) -> Option<UndoStep> {
        self.stored
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .chats
            .get(chat_id)
            .and_then(|stack| stack.last().cloned())
    }

    fn push(&self, chat_id: &str, step: UndoStep) {
        self.change(|stored| {
            let stack = stored.chats.entry(chat_id.to_owned()).or_default();
            stack.push(step);
            let extra = stack.len().saturating_sub(MAX_UNDO);
            stack.drain(..extra);
        });
    }

    fn pop(&self, chat_id: &str) {
        self.change(|stored| {
            if let Some(stack) = stored.chats.get_mut(chat_id) {
                stack.pop();
                if stack.is_empty() {
                    stored.chats.remove(chat_id);
                }
            }
        });
    }

    /// Apply `change` and persist. A failed write is logged, not fatal: the
    /// edit itself already happened, and the stack stays right in memory.
    fn change(&self, change: impl FnOnce(&mut Stored)) {
        let mut stored = self.stored.lock().unwrap_or_else(PoisonError::into_inner);
        change(&mut stored);
        if let Err(err) = self.persist(&stored) {
            tracing::warn!(error = %err, "native history undo stack write failed");
        }
    }

    fn persist(&self, stored: &Stored) -> std::io::Result<()> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer(&mut temp, stored)?;
        temp.write_all(b"\n")?;
        temp.persist(&self.path).map_err(|e| e.error)?;
        Ok(())
    }
}

/// What a chat's next run resumes.
struct Target {
    harness_id: HarnessId,
    harness: Arc<dyn Harness>,
    session_id: String,
    cwd: String,
}

#[derive(Clone)]
pub(crate) struct NativeHistoryService {
    sessions: SessionsEngine,
    doc_host: DocHost,
    workspace: WorkspaceHost,
    registry: Arc<HarnessRegistry>,
    undo: Arc<UndoStore>,
    /// One edit at a time: each rewrites a session and repoints a chat.
    edits: Arc<tokio::sync::Mutex<()>>,
}

impl NativeHistoryService {
    pub(crate) fn new(
        sessions: SessionsEngine,
        doc_host: DocHost,
        workspace: WorkspaceHost,
        registry: Arc<HarnessRegistry>,
        data_dir: &Path,
    ) -> Self {
        Self {
            sessions,
            doc_host,
            workspace,
            registry,
            undo: Arc::new(UndoStore::open(data_dir)),
            edits: Arc::default(),
        }
    }

    /// What the agent's own session holds for the chat.
    pub(crate) async fn get(&self, chat_id: &str) -> Result<NativeHistory, String> {
        let target = self.target(chat_id)?;
        self.read(chat_id, &target).await
    }

    /// Write an edited copy of the chat's session and resume that instead.
    pub(crate) async fn apply(
        &self,
        params: ApplyNativeEditsParams,
    ) -> Result<NativeHistory, String> {
        let chat_id = params.chat_id.as_str();
        let _one_at_a_time = self.edits.lock().await;
        let target = self.target(chat_id)?;
        self.quiesce(chat_id).await?;

        let before = self.read(chat_id, &target).await?;
        if before.revision != params.revision {
            return Err(CHANGED.into());
        }
        let env = self.env(&target).await;
        let new_session = target
            .harness
            .write_edited_session(
                &target.session_id,
                Path::new(&target.cwd),
                &env,
                &params.revision,
                &params.edits,
            )
            .await
            .map_err(|err| refusal(&err, "Couldn't write the edited conversation"))?;

        let restore = self.mirror(chat_id, &before, &params.edits);
        self.undo.push(
            chat_id,
            UndoStep {
                session_id: target.session_id.clone(),
                restore,
            },
        );
        self.switch(chat_id, &target, &target.session_id, &new_session);
        let target = Target {
            session_id: new_session,
            ..target
        };
        self.read(chat_id, &target).await
    }

    /// Resume the session from before the last edit again.
    pub(crate) async fn undo(&self, chat_id: &str) -> Result<NativeHistory, String> {
        let _one_at_a_time = self.edits.lock().await;
        let target = self.target(chat_id)?;
        self.quiesce(chat_id).await?;
        let step = self
            .undo
            .peek(chat_id)
            .ok_or_else(|| "There is nothing to undo".to_string())?;
        let earlier = Target {
            harness_id: target.harness_id,
            harness: target.harness.clone(),
            session_id: step.session_id.clone(),
            cwd: target.cwd.clone(),
        };
        // The earlier copy is the agent's own file: it may be gone.
        let history = self.read(chat_id, &earlier).await.map_err(|_| {
            "The earlier version of this conversation is no longer stored".to_string()
        })?;
        self.switch(chat_id, &earlier, &target.session_id, &step.session_id);
        self.undo.pop(chat_id);
        self.restore_text(chat_id, &step.restore);
        Ok(NativeHistory {
            can_undo: self.undo.can_undo(chat_id),
            ..history
        })
    }

    /// The chat's session, if this device hosts the chat and its agent can be
    /// edited.
    fn target(&self, chat_id: &str) -> Result<Target, String> {
        let chat = self
            .workspace
            .chat(chat_id)
            .map_err(|err| {
                tracing::warn!(chat = %chat_id, error = %err, "native history: chat read failed");
                "Couldn't read this chat".to_string()
            })?
            .ok_or_else(|| "This chat no longer exists".to_string())?;
        if chat.device_id != self.doc_host.device_id() {
            return Err("Edit this conversation on the device that runs it".into());
        }
        let (harness_id, session_id, cwd) = self
            .sessions
            .provider_session(chat_id)
            .ok_or_else(|| "This chat has no agent conversation to edit yet".to_string())?;
        let harness = self.registry.resolve(harness_id).map_err(|err| {
            tracing::warn!(chat = %chat_id, error = %err, "native history: harness unavailable");
            "This agent isn't available on this device".to_string()
        })?;
        if !harness.supports_native_history() {
            return Err(format!(
                "Editing isn't available for {} yet",
                harness.display_name()
            ));
        }
        let cwd = if cwd.is_empty() {
            chat.cwd.unwrap_or_default()
        } else {
            cwd
        };
        Ok(Target {
            harness_id,
            harness,
            session_id,
            cwd,
        })
    }

    /// Nothing may be running, and no idle runtime may keep the old session:
    /// a parked runtime holds it in memory and would answer the next prompt
    /// from it.
    async fn quiesce(&self, chat_id: &str) -> Result<(), String> {
        let busy = || {
            self.sessions.turn_in_flight(chat_id) || self.sessions.holds_background_work(chat_id)
        };
        if busy() {
            return Err(BUSY.into());
        }
        if self.sessions.has_run(chat_id) {
            self.sessions.terminate(chat_id).await.map_err(|err| {
                tracing::warn!(chat = %chat_id, error = %err, "native history: retiring the runtime failed");
                "Couldn't stop the agent's idle session".to_string()
            })?;
            // A prompt sent while the runtime wound down started a new one.
            if busy() || self.sessions.has_run(chat_id) {
                return Err(BUSY.into());
            }
        }
        Ok(())
    }

    /// The environment the agent's runs get here (config dir overrides).
    async fn env(&self, target: &Target) -> std::collections::BTreeMap<String, String> {
        let mut request = RunRequest {
            mcp: None,
            prompt: String::new(),
            harness: Some(target.harness_id),
            model: None,
            reasoning: None,
            model_options: serde_json::Map::new(),
            cwd: target.cwd.clone(),
            sandbox: SandboxLevel::ReadOnly,
            auto_approve: false,
            attachments: Vec::new(),
            worktree: None,
            resume: None,
            env: Default::default(),
            system_prompt: None,
        };
        if let Err(err) = self
            .registry
            .prepare_run(target.harness_id, &mut request)
            .await
        {
            tracing::warn!(error = %err, "native history: run environment unavailable");
        }
        request.env
    }

    async fn read(&self, chat_id: &str, target: &Target) -> Result<NativeHistory, String> {
        let env = self.env(target).await;
        let read = target
            .harness
            .read_native_history(&target.session_id, Path::new(&target.cwd), &env)
            .await
            .map_err(|err| refusal(&err, UNREADABLE))?;
        Ok(NativeHistory {
            chat_id: chat_id.to_owned(),
            harness: target.harness_id.as_str().to_owned(),
            session_id: target.session_id.clone(),
            revision: read.revision,
            items: read.items,
            compacted: read.compacted,
            can_undo: self.undo.can_undo(chat_id),
        })
    }

    /// Point the chat at `to` instead of `from`, and keep the bookkeeping that
    /// names the session in step: a side chat or switched chat records which
    /// session already got its replayed history.
    fn switch(&self, chat_id: &str, target: &Target, from: &str, to: &str) {
        self.sessions
            .repoint_provider_session(chat_id, to, &target.cwd, target.harness_id);
        let Ok(handle) = self.doc_host.open(chat_id) else {
            return;
        };
        let doc = handle.doc();
        if doc.fork_history_session().as_deref() == Some(from) {
            let _ = doc.set_fork_history_session(to);
        }
        let harness = target.harness_id.as_str();
        if let Some(mark) = doc.provider_session(harness)
            && mark.session_id == from
        {
            let _ = doc.set_provider_session(
                harness,
                &ProviderSessionMark {
                    session_id: to.to_owned(),
                    ..mark
                },
            );
        }
    }

    /// Make Zeron's own transcript say what the agent now believes. Best
    /// effort: a part that can't be matched unambiguously is left alone and
    /// the edit never fails over it. Answers what changed, for undo.
    fn mirror(
        &self,
        chat_id: &str,
        before: &NativeHistory,
        edits: &[NativeEdit],
    ) -> Vec<TextRestore> {
        let Ok(handle) = self.doc_host.open(chat_id) else {
            return Vec::new();
        };
        let doc = handle.doc();
        let mut restore = Vec::new();
        for edit in edits {
            let id = match edit {
                NativeEdit::Replace { id, .. } | NativeEdit::Remove { id } => id,
            };
            let Some(item) = before.items.iter().find(|item| &item.id == id) else {
                continue;
            };
            let role = match item.role {
                NativeRole::User => MessageRole::User,
                NativeRole::Assistant => MessageRole::Assistant,
            };
            let changed = match (edit, &item.kind) {
                (NativeEdit::Replace { text, .. }, NativeItemKind::Text { text: old }) => doc
                    .rewrite_unique_part_text(role, "text", old, Some(text))
                    .map(|changed| {
                        if changed {
                            restore.push(TextRestore {
                                role: role_name(role).into(),
                                current: text.clone(),
                                original: old.clone(),
                            });
                        }
                    }),
                (
                    NativeEdit::Remove { .. },
                    NativeItemKind::Reasoning {
                        text: Some(old), ..
                    },
                ) => doc
                    .rewrite_unique_part_text(role, "reasoning", old, None)
                    .map(|_| ()),
                _ => Ok(()),
            };
            if let Err(err) = changed {
                tracing::debug!(chat = %chat_id, error = %err, "native history: transcript mirror skipped");
            }
        }
        restore
    }

    /// The transcript texts an undone edit had changed, put back.
    fn restore_text(&self, chat_id: &str, restore: &[TextRestore]) {
        let Ok(handle) = self.doc_host.open(chat_id) else {
            return;
        };
        for step in restore {
            let role = if step.role == "user" {
                MessageRole::User
            } else {
                MessageRole::Assistant
            };
            if let Err(err) = handle.doc().rewrite_unique_part_text(
                role,
                "text",
                &step.current,
                Some(&step.original),
            ) {
                tracing::debug!(chat = %chat_id, error = %err, "native history: transcript restore skipped");
            }
        }
    }
}

fn role_name(role: MessageRole) -> &'static str {
    match role {
        MessageRole::User => "user",
        _ => "assistant",
    }
}

/// A harness error as the message the person editing sees: its own words
/// when it wrote them for them, else `fallback` (with the cause logged).
fn refusal(err: &HarnessError, fallback: &str) -> String {
    match err {
        HarnessError::History(message) => message.clone(),
        other => {
            tracing::warn!(error = %other, "native history: {fallback}");
            fallback.to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str) -> UndoStep {
        UndoStep {
            session_id: id.into(),
            restore: Vec::new(),
        }
    }

    #[test]
    fn the_undo_stack_is_per_chat_bounded_and_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = UndoStore::open(dir.path());
        assert!(!store.can_undo("a"));
        store.push("a", step("s1"));
        store.push("a", step("s2"));
        store.push("b", step("other"));
        assert_eq!(store.peek("a").unwrap().session_id, "s2");

        let reopened = UndoStore::open(dir.path());
        assert_eq!(reopened.peek("a").unwrap().session_id, "s2");
        reopened.pop("a");
        assert_eq!(reopened.peek("a").unwrap().session_id, "s1");
        reopened.pop("a");
        assert!(!reopened.can_undo("a"));
        assert!(reopened.can_undo("b"));

        for n in 0..MAX_UNDO + 5 {
            reopened.push("c", step(&format!("s{n}")));
        }
        let kept = reopened.stored.lock().unwrap().chats["c"].len();
        assert_eq!(kept, MAX_UNDO);
        assert_eq!(
            reopened.peek("c").unwrap().session_id,
            format!("s{}", MAX_UNDO + 4)
        );
    }
}
