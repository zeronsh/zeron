//! The engine as plugins' agent ops reach it (`plugins::agents::AgentHost`):
//! the same calls Settings and the composer make, on this device.
//!
//! - Accounts: `AgentAccounts::list` / `activate`, as `ListAgentAccounts` and
//!   `ActivateAgentAccount` answer (a switch retires the voice session
//!   first, as that method does).
//! - Models: the harness's catalog, as `ListModels` lists it.
//! - A chat's model: `WorkspaceHost::set_chat_config`, as `Mutate
//!   setChatConfig` writes it for a change that keeps the chat's provider.
//! - A message (`messages.send`): `QueueCommand run` / `steer`, or a
//!   `QueueMessage` row held for the turn's end, into this device's copy of
//!   the chat, as the composer sends (its host, here or elsewhere, takes it
//!   as it takes the composer's); a new chat as `Mutate createChat` makes
//!   one; an answer as `respond_to_input` queues it.

use std::path::PathBuf;
use std::sync::Arc;

use futures::future::BoxFuture;
use zeron_doc::SessionCommandPayload;
use zeron_proto::{AgentAccount, AgentAccountsSnapshot, Chat, ChatConfig, HarnessId, Model, Session, SessionStatus, UserInputAnswer};

use crate::agent_accounts::AgentAccounts;
use crate::plugins::agents::{AgentHost, Delivery, NewChat, Pin};
use crate::registry::HarnessRegistry;
use crate::sessions::SessionsEngine;
use crate::workspace_host::WorkspaceHost;

pub struct EngineAgentHost {
    pub accounts: AgentAccounts,
    pub registry: Arc<HarnessRegistry>,
    pub data_dir: PathBuf,
    pub workspace: WorkspaceHost,
    pub sessions: SessionsEngine,
    /// Each chat's doc here: commands and queued rows go in as the
    /// composer's do.
    pub doc_host: crate::DocHost,
    /// What a new chat runs when a plugin names no harness.
    pub default_harness: HarnessId,
}

impl AgentHost for EngineAgentHost {
    fn accounts(&self, force: bool) -> BoxFuture<'static, Result<AgentAccountsSnapshot, String>> {
        let accounts = self.accounts.clone();
        Box::pin(async move { accounts.list(force).await.map_err(|e| e.to_string()) })
    }

    fn activate(&self, harness: HarnessId, account: String) -> BoxFuture<'static, Result<AgentAccountsSnapshot, String>> {
        let (accounts, voice) = (self.accounts.clone(), self.sessions.voice_manager());
        Box::pin(async move {
            voice.retire();
            accounts.activate(harness, &account).await.map_err(|e| e.to_string())
        })
    }

    fn models(&self, harness: HarnessId) -> BoxFuture<'static, Result<Vec<Model>, String>> {
        let (registry, data_dir) = (self.registry.clone(), self.data_dir.clone());
        Box::pin(async move {
            let lease = Arc::new(registry.execution_lease(harness).await);
            let resolved = registry.resolve(harness).map_err(|e| e.to_string())?;
            crate::model_catalogs::list_with_lease(&data_dir, resolved, false, Some(lease))
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn chats(&self) -> Vec<Chat> {
        self.workspace.read_chats().unwrap_or_default()
    }

    fn sessions(&self) -> Vec<Session> {
        self.sessions.watch_sessions().borrow().clone()
    }

    fn set_chat_config(&self, chat: &str, config: ChatConfig) -> Result<(), String> {
        self.sessions.voice_manager().retire_chat(chat);
        self.workspace.set_chat_config(chat, &config).map(drop).map_err(|e| e.to_string())
    }

    fn status(&self, chat: &str) -> Option<SessionStatus> {
        // This device's own, else the row its host mirrors to the workspace.
        let here = self.sessions.watch_sessions().borrow().iter().find(|s| s.chat_id == chat).map(|s| s.status);
        here.or_else(|| {
            self.workspace
                .read_sessions()
                .ok()?
                .into_iter()
                .find(|s| s.chat_id == chat)
                .map(|s| s.status)
        })
    }

    fn deliver(&self, chat: &str, text: &str, attachments: Vec<String>, how: Delivery) -> Result<String, String> {
        let failed = |e: crate::EngineError| e.to_string();
        match how {
            // (As the composer's `QueueMessage`: held for the turn's end.)
            Delivery::Queue => self.doc_host.queue_message_with_behavior(chat, text, attachments, true).map_err(failed),
            Delivery::Steer => {
                if !attachments.is_empty() {
                    return Err("A steer carries words only: queue a message with files".into());
                }
                let id = crate::new_id();
                let payload = SessionCommandPayload::Steer { prompt: text.to_owned(), message_id: Some(id.clone()) };
                self.doc_host.queue_command_with_transfers(chat, payload, Vec::new()).map_err(failed)?;
                Ok(id)
            }
            Delivery::Run => {
                // The chat's own config, as the composer runs it.
                let mut request = self
                    .doc_host
                    .request_from_chat_row(chat, text)
                    .ok_or_else(|| format!("There's no chat {chat}"))?;
                if request.cwd.is_empty() {
                    request.cwd = "~".into();
                }
                request.attachments = attachments;
                let id = crate::new_id();
                let payload = SessionCommandPayload::Run { request, message_id: id.clone() };
                self.doc_host.queue_command_with_transfers(chat, payload, Vec::new()).map_err(failed)?;
                Ok(id)
            }
        }
    }

    fn pin_mechanism(&self, account: &AgentAccount) -> Option<&'static str> {
        self.accounts.pin_mechanism(account)
    }

    fn pins(&self) -> Vec<Pin> {
        self.accounts
            .pins()
            .into_iter()
            .map(|p| Pin { chat: p.chat, harness: p.harness, account: p.account, since: p.since })
            .collect()
    }

    fn set_pin(&self, harness: HarnessId, chat: &str, account: Option<&str>) -> Result<(), String> {
        self.accounts.set_pin(harness, chat, account);
        Ok(())
    }

    fn create_chat(&self, new: &NewChat) -> Result<(), String> {
        if new.device.as_deref().is_some_and(|d| zeron_proto::is_cloud_device(d, "")) {
            return Err("A Cloud chat is started from the composer".into());
        }
        self.workspace
            .create_chat_hosted(
                &new.chat_id,
                new.project.as_deref(),
                new.device.as_deref(),
                None,
                Some(new.config.clone()),
                new.cwd.clone(),
                None,
                None,
            )
            .map_err(|e| e.to_string())
    }

    fn pending_input(&self, chat: &str) -> Result<Option<String>, String> {
        self.doc_host.pending_input(chat).map_err(|e| e.to_string())
    }

    fn answer(&self, chat: &str, request: &str, answers: Vec<UserInputAnswer>) -> Result<String, String> {
        let payload = SessionCommandPayload::RespondInput { request_id: request.to_owned(), answers };
        self.doc_host.queue_command_with_transfers(chat, payload, Vec::new()).map_err(|e| e.to_string())
    }

    fn mark_sent_by(&self, chat: &str, id: &str, by: serde_json::Value) {
        if let Err(error) = self.doc_host.mark_sent_by(chat, id, by) {
            tracing::warn!(%chat, %error, "plugins: who sent a message wasn't recorded");
        }
    }

    fn default_harness(&self) -> Option<HarnessId> {
        Some(self.default_harness)
    }
}
