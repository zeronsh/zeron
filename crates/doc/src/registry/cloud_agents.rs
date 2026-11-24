//! What the person chose for an agent on Cloud (Settings → Providers, Cloud):
//! whether it keeps its own subagents, runs in Code Mode, and gets only
//! Zeron's MCP servers.
//!
//! A Cloud chat runs on a machine of its own, made for it and gone after, so
//! a choice kept on one would be lost with it. A `cloudAgents` row per agent
//! (its id the agent's, `claude-code`) keeps them for the account, per-field
//! LWW like every row: the person's devices write them, and every Cloud
//! machine reads them (the room shows Cloud runners this kind, never lets
//! them write it: edge/src/registry-runner.ts). A field never written is no
//! choice, and the machine runs the agent as it would anywhere.

use super::*;

pub const KIND_CLOUD_AGENTS: &str = "cloudAgents";

/// One choice of a Cloud agent's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudAgentChoice {
    /// Keeps its provider's own subagents (off: delegates through side chats).
    NativeSubagents,
    CodeMode,
    /// Only Zeron's MCP servers, its own and its account's connectors hidden.
    ZeronMcpOnly,
}

impl CloudAgentChoice {
    /// Its field on the row (edge/src/registry-core.ts validates these).
    pub fn field(self) -> &'static str {
        match self {
            Self::NativeSubagents => "nativeSubagents",
            Self::CodeMode => "codeMode",
            Self::ZeronMcpOnly => "zeronMcpOnly",
        }
    }
}

/// An agent's choices on Cloud; `None` where none was made.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CloudAgentChoices {
    pub native_subagents: Option<bool>,
    pub code_mode: Option<bool>,
    pub zeron_mcp_only: Option<bool>,
}

impl CloudAgentChoices {
    fn of_row(row: &RegistryRow) -> Self {
        let read = |choice: CloudAgentChoice| row.fields.get(choice.field()).and_then(Value::as_bool);
        Self {
            native_subagents: read(CloudAgentChoice::NativeSubagents),
            code_mode: read(CloudAgentChoice::CodeMode),
            zeron_mcp_only: read(CloudAgentChoice::ZeronMcpOnly),
        }
    }

    pub fn get(&self, choice: CloudAgentChoice) -> Option<bool> {
        match choice {
            CloudAgentChoice::NativeSubagents => self.native_subagents,
            CloudAgentChoice::CodeMode => self.code_mode,
            CloudAgentChoice::ZeronMcpOnly => self.zeron_mcp_only,
        }
    }
}

impl RegistryDoc {
    /// `agent`'s choices on Cloud (its serialized id, `claude-code`).
    pub fn cloud_agent_choices(&self, agent: &str) -> CloudAgentChoices {
        self.overlay_row(KIND_CLOUD_AGENTS, agent)
            .map(|row| CloudAgentChoices::of_row(&row))
            .unwrap_or_default()
    }

    /// Every agent with a choice made on Cloud, by id.
    pub fn all_cloud_agent_choices(&self) -> Vec<(String, CloudAgentChoices)> {
        self.overlay_rows(KIND_CLOUD_AGENTS)
            .into_iter()
            .map(|row| {
                let choices = CloudAgentChoices::of_row(&row);
                (row.id, choices)
            })
            .collect()
    }

    /// Make one of `agent`'s choices on Cloud. Nothing is written when it is
    /// already so.
    pub fn set_cloud_agent_choice(&mut self, agent: &str, choice: CloudAgentChoice, on: bool) {
        if self.cloud_agent_choices(agent).get(choice) == Some(on) {
            return;
        }
        // (An upsert: the row's first choice makes it; the others it holds
        // keep their own clocks.)
        self.write(KIND_CLOUD_AGENTS, agent, OpKind::Upsert, fields([(choice.field(), json!(on))]));
    }
}
