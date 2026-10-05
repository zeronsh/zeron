//! What an agent may do (docs/plans/2026-09-30-agent-mobility-and-policy.md,
//! Part 4): a permission mode, a sandbox, and the user's standing rules.
//!
//! The policy rides every run (`RunRequest.policy`) and is the chat's
//! setting (`ChatConfig.policy`). Harnesses enforce it with their own native
//! modes where they have them, and answer the approval requests they receive
//! through Zeron's shared decision (`zeron_harness::policy`); a question the
//! policy can't settle reaches the user through the ordinary question panel.

use serde::{Deserialize, Serialize};

/// How much the agent may do without asking. Ordered from most to least
/// permissive except `Plan`, which is read-only by design.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    /// Everything is allowed (Zeron's behaviour before modes existed).
    #[default]
    Bypass,
    /// Safe actions are allowed, risky ones ask, destructive ones are
    /// refused; standing rules decide first.
    Auto,
    /// Reads, read-only commands, and workspace edits are allowed; other
    /// commands and edits outside the workspace ask.
    AcceptEdits,
    /// Reads and read-only commands are allowed; everything else asks.
    Ask,
    /// Read-only: the agent investigates and proposes a plan; edits and
    /// other commands are refused until the plan is approved.
    Plan,
}

impl PermissionMode {
    pub const ALL: [PermissionMode; 5] = [
        PermissionMode::Bypass,
        PermissionMode::Auto,
        PermissionMode::AcceptEdits,
        PermissionMode::Ask,
        PermissionMode::Plan,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PermissionMode::Bypass => "Bypass permissions",
            PermissionMode::Auto => "Auto",
            PermissionMode::AcceptEdits => "Accept edits",
            PermissionMode::Ask => "Ask",
            PermissionMode::Plan => "Plan",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            PermissionMode::Bypass => "Runs everything without asking",
            PermissionMode::Auto => {
                "Safe actions run, risky ones ask, destructive ones are refused"
            }
            PermissionMode::AcceptEdits => "Reads and project edits run; other commands ask",
            PermissionMode::Ask => "Reads run; edits and other commands ask",
            PermissionMode::Plan => "Read-only: proposes a plan before changing anything",
        }
    }
}

/// What the agent's process may touch, enforced below the agent (the OS
/// sandbox, or the harness's own sandbox where it has one).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SandboxMode {
    /// No sandbox.
    #[default]
    Off,
    /// Writes only inside the workspace, temp folders and the agent's own
    /// state; reads anywhere.
    WorkspaceWrite,
    /// No writes outside temp folders and the agent's own state.
    ReadOnly,
}

impl SandboxMode {
    pub fn label(self) -> &'static str {
        match self {
            SandboxMode::Off => "No sandbox",
            SandboxMode::WorkspaceWrite => "Workspace write",
            SandboxMode::ReadOnly => "Read-only",
        }
    }
}

/// The kind of thing an agent wants to do, as far as a policy cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActionKind {
    Read,
    Edit,
    Exec,
    Network,
    /// A tool of an MCP server.
    Mcp,
    Other,
}

/// What a standing rule does when it matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuleEffect {
    Allow,
    Ask,
    Deny,
}

/// One standing rule ("always allow `cargo test`", "never touch `deploy/`").
/// Rules are checked in order; the first match decides. A pattern is a glob
/// (`*` any run of characters, `?` one) matched against the command for
/// `Exec`, each path for `Read`/`Edit`, the host for `Network`, and
/// `server__tool` for `Mcp`. `kind: None` matches any kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ActionKind>,
    pub pattern: String,
    pub effect: RuleEffect,
}

/// A chat's (or a run's) policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPolicy {
    #[serde(default)]
    pub mode: PermissionMode,
    #[serde(default)]
    pub sandbox: SandboxMode,
    /// The sandbox lets the agent reach the network.
    #[serde(default = "default_true")]
    pub network: bool,
    /// Standing rules, decided before the mode (filled in by the host from
    /// the user's and the project's rules at dispatch).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<PolicyRule>,
    /// Nobody is there to answer (a headless child ask, a goal's verifier):
    /// anything the policy would ask about is refused with a reason instead,
    /// so the run keeps going rather than parking on a question forever.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unattended: bool,
}

impl Default for AgentPolicy {
    fn default() -> Self {
        Self {
            mode: PermissionMode::Bypass,
            sandbox: SandboxMode::Off,
            network: true,
            rules: Vec::new(),
            unattended: false,
        }
    }
}

impl AgentPolicy {
    pub fn with_mode(mode: PermissionMode) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }

    /// Read-only: investigates without changing anything (Plan's rules,
    /// and a read-only sandbox where one is enforced).
    pub fn read_only() -> Self {
        Self {
            mode: PermissionMode::Plan,
            sandbox: SandboxMode::ReadOnly,
            ..Self::default()
        }
    }

    /// The agent can't change the workspace: Plan mode or a read-only sandbox.
    pub fn is_read_only(&self) -> bool {
        self.mode == PermissionMode::Plan || self.sandbox == SandboxMode::ReadOnly
    }

    /// The same policy capped by `ceiling`: never more permissive than it in
    /// mode, sandbox or network (an agent-spawned chat under its spawner).
    /// Standing rules carry over from both; unattended if either is.
    pub fn capped_by(&self, ceiling: &AgentPolicy) -> AgentPolicy {
        let mut rules = ceiling.rules.clone();
        rules.extend(self.rules.iter().cloned());
        AgentPolicy {
            mode: stricter_mode(self.mode, ceiling.mode),
            sandbox: stricter_sandbox(self.sandbox, ceiling.sandbox),
            network: self.network && ceiling.network,
            rules,
            unattended: self.unattended || ceiling.unattended,
        }
    }
}

fn mode_rank(mode: PermissionMode) -> u8 {
    match mode {
        PermissionMode::Bypass => 0,
        PermissionMode::Auto => 1,
        PermissionMode::AcceptEdits => 2,
        PermissionMode::Ask => 3,
        PermissionMode::Plan => 4,
    }
}

pub fn stricter_mode(a: PermissionMode, b: PermissionMode) -> PermissionMode {
    if mode_rank(a) >= mode_rank(b) { a } else { b }
}

fn sandbox_rank(sandbox: SandboxMode) -> u8 {
    match sandbox {
        SandboxMode::Off => 0,
        SandboxMode::WorkspaceWrite => 1,
        SandboxMode::ReadOnly => 2,
    }
}

pub fn stricter_sandbox(a: SandboxMode, b: SandboxMode) -> SandboxMode {
    if sandbox_rank(a) >= sandbox_rank(b) {
        a
    } else {
        b
    }
}

fn default_true() -> bool {
    true
}

/// What a harness can honour, for pickers to offer only what works.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyCaps {
    pub modes: Vec<PermissionMode>,
    pub sandboxes: Vec<SandboxMode>,
    /// The harness has its own plan mode (otherwise Zeron emulates it).
    #[serde(default)]
    pub native_plan: bool,
    /// The mode can change on a live session without restarting it.
    #[serde(default)]
    pub live_mode_switch: bool,
}

impl Default for PolicyCaps {
    fn default() -> Self {
        Self::bypass_only()
    }
}

impl PolicyCaps {
    /// A harness whose agent never asks permission: only `Bypass` is honest
    /// (until an OS sandbox confines it).
    pub fn bypass_only() -> Self {
        Self {
            modes: vec![PermissionMode::Bypass],
            sandboxes: vec![SandboxMode::Off],
            native_plan: false,
            live_mode_switch: false,
        }
    }

    pub fn all_modes() -> Self {
        Self {
            modes: PermissionMode::ALL.to_vec(),
            sandboxes: vec![SandboxMode::Off],
            native_plan: false,
            live_mode_switch: false,
        }
    }

    pub fn supports(&self, mode: PermissionMode) -> bool {
        self.modes.contains(&mode)
    }

    /// Why `harness` (its display name) can't run in `mode`; `None` when it
    /// can. Pickers show this beside a greyed mode.
    pub fn unsupported_reason(&self, harness: &str, mode: PermissionMode) -> Option<String> {
        if self.supports(mode) {
            return None;
        }
        Some(if self.modes.iter().all(|m| *m == PermissionMode::Bypass) {
            format!("{harness} runs without asking — it can only bypass permissions")
        } else {
            format!("{harness} can't run in {} mode", mode.label())
        })
    }

    /// The error a host gives a run asking `harness` for a mode it can't
    /// honour. Hosts refuse such runs rather than silently loosening them.
    pub fn refusal(&self, harness: &str, mode: PermissionMode) -> Option<String> {
        let reason = self.unsupported_reason(harness, mode)?;
        let offered: Vec<&str> = PermissionMode::ALL
            .iter()
            .filter(|m| self.supports(**m))
            .map(|m| m.label())
            .collect();
        Some(if offered.is_empty() {
            format!("{reason}.")
        } else {
            format!("{reason}. Pick {} for it.", offered.join(" or "))
        })
    }
}

/// Question ids of approval prompts start with this, so the host can tell
/// them from the agent's own questions (and remember "always" answers).
pub const APPROVAL_QUESTION_PREFIX: &str = "approval:";
/// The options of an approval prompt, in order.
pub const APPROVAL_ALLOW_ONCE: &str = "Allow once";
pub const APPROVAL_ALLOW_ALWAYS: &str = "Always allow";
pub const APPROVAL_DENY: &str = "Deny";

/// The id of an approval question: the prefix, a nonce (no `:`), and — when
/// "Always allow" can become a standing rule — that rule as JSON. The rule
/// rides the id so the host that resolves the answer can remember it without
/// a side channel (and without widening `UserInputQuestion`, which every
/// harness builds by hand).
pub fn approval_question_id(nonce: &str, rule: Option<&PolicyRule>) -> String {
    let nonce = nonce.replace(':', "-");
    match rule.and_then(|rule| serde_json::to_string(rule).ok()) {
        Some(rule) => format!("{APPROVAL_QUESTION_PREFIX}{nonce}:{rule}"),
        None => format!("{APPROVAL_QUESTION_PREFIX}{nonce}"),
    }
}

pub fn is_approval_question(question_id: &str) -> bool {
    question_id.starts_with(APPROVAL_QUESTION_PREFIX)
}

/// The standing rule an approval question's "Always allow" adds, if its id
/// carries one (see [`approval_question_id`]).
pub fn approval_rule(question_id: &str) -> Option<PolicyRule> {
    let rest = question_id.strip_prefix(APPROVAL_QUESTION_PREFIX)?;
    let (_, rule) = rest.split_once(':')?;
    serde_json::from_str(rule).ok()
}

/// Whether a user's answer to an approval question was "Always allow".
pub fn approval_answer_is_always(labels: &[String]) -> bool {
    labels.first().map(String::as_str) == Some(APPROVAL_ALLOW_ALWAYS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_peer_without_a_policy_reads_as_bypass() {
        let policy: AgentPolicy = serde_json::from_str("{}").unwrap();
        assert_eq!(policy, AgentPolicy::default());
        assert_eq!(policy.mode, PermissionMode::Bypass);
        assert!(policy.network);
    }

    #[test]
    fn capping_never_loosens() {
        let child = AgentPolicy::with_mode(PermissionMode::Bypass);
        let parent = AgentPolicy {
            mode: PermissionMode::Ask,
            sandbox: SandboxMode::WorkspaceWrite,
            network: false,
            rules: vec![],
            unattended: true,
        };
        let capped = child.capped_by(&parent);
        assert_eq!(capped.mode, PermissionMode::Ask);
        assert_eq!(capped.sandbox, SandboxMode::WorkspaceWrite);
        assert!(!capped.network);
        assert!(capped.unattended, "an unattended ceiling stays unattended");
        let strict = AgentPolicy::with_mode(PermissionMode::Plan);
        assert_eq!(strict.capped_by(&parent).mode, PermissionMode::Plan);
        assert!(AgentPolicy::read_only().is_read_only());
        assert!(!AgentPolicy::default().is_read_only());
    }

    #[test]
    fn unsupported_modes_explain_themselves() {
        let bypass = PolicyCaps::bypass_only();
        assert_eq!(
            bypass.unsupported_reason("Cursor", PermissionMode::Bypass),
            None
        );
        assert_eq!(
            bypass
                .unsupported_reason("Cursor", PermissionMode::Ask)
                .as_deref(),
            Some("Cursor runs without asking — it can only bypass permissions")
        );
        assert_eq!(
            bypass.refusal("Cursor", PermissionMode::Plan).as_deref(),
            Some(
                "Cursor runs without asking — it can only bypass permissions. \
                 Pick Bypass permissions for it."
            )
        );
        let some = PolicyCaps {
            modes: vec![PermissionMode::Bypass, PermissionMode::Ask],
            ..PolicyCaps::bypass_only()
        };
        assert_eq!(
            some.refusal("Codex", PermissionMode::Plan).as_deref(),
            Some("Codex can't run in Plan mode. Pick Bypass permissions or Ask for it.")
        );
        assert_eq!(
            PolicyCaps::all_modes().refusal("Claude", PermissionMode::Auto),
            None
        );
    }

    #[test]
    fn approval_ids_carry_their_rule() {
        let rule = PolicyRule {
            kind: Some(ActionKind::Exec),
            pattern: "cargo test: \"all\"".into(),
            effect: RuleEffect::Allow,
        };
        let id = approval_question_id("a:b", Some(&rule));
        assert!(is_approval_question(&id));
        assert_eq!(approval_rule(&id), Some(rule));
        let bare = approval_question_id("n1", None);
        assert_eq!(bare, "approval:n1");
        assert_eq!(approval_rule(&bare), None);
        assert_eq!(approval_rule("q-sync"), None);
        assert!(approval_answer_is_always(&[
            APPROVAL_ALLOW_ALWAYS.to_string()
        ]));
        assert!(!approval_answer_is_always(&[
            APPROVAL_ALLOW_ONCE.to_string()
        ]));
    }
}
