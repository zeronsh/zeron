//! Agent presets (docs/agent-presets.md): a named agent — a harness, model and
//! permission policy plus instructions and tool limits — to pick when starting
//! a chat or to have an orchestrating agent spawn by name.

use serde::{Deserialize, Serialize};

use crate::{AgentPolicy, HarnessId, ReasoningLevel};

/// Where a preset was defined. A project's own preset beats a user preset of
/// the same id inside that project.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PresetSource {
    /// The device's `presets.json`; created and edited in Settings.
    #[default]
    User,
    /// `.zeron/agents/<id>.md` in the project, checked in with it.
    Project,
    /// A Claude Code subagent file (`.claude/agents/*.md`), offered read-only.
    Imported,
}

/// What to use when a preset's harness isn't usable on the device it runs on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetFallback {
    pub harness: HarnessId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Which of Zeron's own MCP tools a preset's agent is offered. An empty
/// `allow` means "all of them"; `deny` always wins.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetTools {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
}

impl PresetTools {
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }

    pub fn permits(&self, tool: &str) -> bool {
        !self.deny.iter().any(|t| t == tool)
            && (self.allow.is_empty() || self.allow.iter().any(|t| t == tool))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPreset {
    /// Stable key: lowercase letters, digits and dashes.
    pub id: String,
    pub name: String,
    /// Tells an orchestrating agent when to use this preset.
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub harness: HarnessId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningLevel>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub model_options: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub policy: AgentPolicy,
    /// Added to the agent's system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "PresetTools::is_empty")]
    pub tools: PresetTools,
    /// Start in a fresh worktree.
    #[serde(default)]
    pub worktree: bool,
    /// May create chats itself (the Zeron MCP `create_chat` tools).
    #[serde(default = "yes")]
    pub may_spawn: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<PresetFallback>,
    #[serde(default)]
    pub source: PresetSource,
}

fn yes() -> bool {
    true
}

impl AgentPreset {
    pub fn new(name: &str, harness: HarnessId) -> Self {
        Self {
            id: slug(name),
            name: name.trim().to_string(),
            description: String::new(),
            icon: None,
            color: None,
            harness,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            policy: AgentPolicy::default(),
            instructions: None,
            tools: PresetTools::default(),
            worktree: false,
            may_spawn: true,
            fallbacks: Vec::new(),
            source: PresetSource::User,
        }
    }

    /// A short stable fingerprint of what the preset asks for, so a chat can
    /// tell its preset was edited since the chat started. The source and the
    /// description (which only orchestrators read) don't count.
    pub fn digest(&self) -> String {
        let mut canonical = self.clone();
        canonical.source = PresetSource::User;
        canonical.description.clear();
        let json = serde_json::to_string(&canonical).unwrap_or_default();
        // FNV-1a, 64 bits: stable across builds and platforms, unlike the
        // standard library's hashers.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in json.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{hash:016x}")
    }

    /// What a chat keeps of the preset it started from.
    pub fn reference(&self) -> PresetRef {
        PresetRef {
            id: self.id.clone(),
            name: self.name.clone(),
            digest: self.digest(),
            instructions: self.instructions.clone().filter(|i| !i.trim().is_empty()),
            may_spawn: self.may_spawn,
            tools: self.tools.clone(),
        }
    }
}

/// A chat's record of its preset: enough to keep running as it was started,
/// whatever happens to the preset afterwards. Editing a preset never changes
/// a running chat; the chat offers to update when `digest` no longer matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetRef {
    pub id: String,
    pub name: String,
    pub digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default = "yes")]
    pub may_spawn: bool,
    #[serde(default, skip_serializing_if = "PresetTools::is_empty")]
    pub tools: PresetTools,
}

/// `Reviewer (strict)` → `reviewer-strict`.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PermissionMode;

    #[test]
    fn names_become_ids() {
        assert_eq!(slug("Reviewer (strict)"), "reviewer-strict");
        assert_eq!(slug("  Fast  fixer "), "fast-fixer");
        assert_eq!(slug("日本語"), "");
    }

    #[test]
    fn an_old_or_sparse_file_reads_with_sensible_defaults() {
        let preset: AgentPreset = serde_json::from_value(serde_json::json!({
            "id": "reviewer", "name": "Reviewer", "harness": "codex"
        }))
        .unwrap();
        assert_eq!(preset.policy.mode, PermissionMode::Bypass);
        assert!(preset.may_spawn, "spawning is allowed unless a preset says no");
        assert_eq!(preset.source, PresetSource::User);
        assert!(preset.tools.permits("anything"));
    }

    #[test]
    fn the_digest_follows_what_the_agent_does_not_the_label() {
        let mut preset = AgentPreset::new("Reviewer", HarnessId::Codex);
        let digest = preset.digest();
        preset.description = "Reviews diffs".into();
        preset.source = PresetSource::Project;
        assert_eq!(preset.digest(), digest, "description and source don't count");
        preset.policy.mode = PermissionMode::Ask;
        assert_ne!(preset.digest(), digest);
        assert_eq!(digest.len(), 16);
    }

    #[test]
    fn a_reference_keeps_what_the_chat_needs_to_keep_running() {
        let mut preset = AgentPreset::new("Reviewer", HarnessId::Codex);
        preset.instructions = Some("  \n".into());
        assert_eq!(preset.reference().instructions, None, "blank is no instructions");
        preset.instructions = Some("Only report; never edit.".into());
        preset.may_spawn = false;
        let reference = preset.reference();
        assert_eq!(reference.instructions.as_deref(), Some("Only report; never edit."));
        assert!(!reference.may_spawn);
        assert_eq!(reference.digest, preset.digest());
    }

    #[test]
    fn tool_limits_deny_first_then_allow() {
        let tools = PresetTools {
            allow: vec!["read_chat".into(), "get_chat".into()],
            deny: vec!["get_chat".into()],
        };
        assert!(tools.permits("read_chat"));
        assert!(!tools.permits("get_chat"));
        assert!(!tools.permits("create_chat"));
        assert!(PresetTools::default().permits("create_chat"));
    }
}
