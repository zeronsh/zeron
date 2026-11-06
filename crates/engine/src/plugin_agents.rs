//! The engine as plugins' agent ops reach it (`plugins::agents::AgentHost`):
//! the same calls Settings and the composer make, on this device.
//!
//! - Accounts: `AgentAccounts::list` / `activate`, as `ListAgentAccounts` and
//!   `ActivateAgentAccount` answer (a switch retires the voice session
//!   first, as that method does).
//! - Models: the harness's catalog, as `ListModels` lists it.
//! - A chat's model: `WorkspaceHost::set_chat_config`, as `Mutate
//!   setChatConfig` writes it for a change that keeps the chat's provider.

use std::path::PathBuf;
use std::sync::Arc;

use futures::future::BoxFuture;
use zeron_proto::{AgentAccountsSnapshot, Chat, ChatConfig, HarnessId, Model, Session};

use crate::agent_accounts::AgentAccounts;
use crate::plugins::agents::AgentHost;
use crate::registry::HarnessRegistry;
use crate::sessions::SessionsEngine;
use crate::workspace_host::WorkspaceHost;

pub struct EngineAgentHost {
    pub accounts: AgentAccounts,
    pub registry: Arc<HarnessRegistry>,
    pub data_dir: PathBuf,
    pub workspace: WorkspaceHost,
    pub sessions: SessionsEngine,
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
}
