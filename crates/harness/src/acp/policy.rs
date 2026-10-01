//! Zeron's permission policy over ACP (docs/plans/2026-09-30-agent-mobility-
//! and-policy.md, Part 4): which of the agent's own modes a run selects, and
//! how a `session/request_permission` is answered outside Bypass.
//!
//! | Agent | Mode surface | Bypass | Auto / Accept edits / Ask | Plan |
//! | --- | --- | --- | --- | --- |
//! | Devin | `mode` config select | `bypass` | `normal` | `plan` |
//! | Antigravity | `mode` config select (+ legacy modes) | `yolo` | `default` | `default` (no plan mode; the policy refuses edits) |
//! | Hermes | legacy modes (`default`/`accept_edits`/`dont_ask`) | untouched | `default` | `default` |
//! | Grok | none over ACP (`[ui] permission_mode` in its own config) | untouched | untouched | untouched |
//!
//! Every mode other than Bypass picks the agent's most-asking mode rather
//! than its own "accept edits"/"smart" equivalents, so each action reaches
//! the policy (and the user's standing rules) instead of the agent's
//! judgment. Devin's mode values are read from its 3000.11.3 binary;
//! Antigravity's from agy_acp_server 1.2.1's `config_options.py`; Hermes'
//! from `acp_adapter/server.py`; Grok 1.0.44's `session/new` advertises
//! only `model` and `reasoning_effort` (probed live).

use std::path::PathBuf;

use serde_json::Value;
use zeron_proto::{ActionKind, PermissionMode, PolicyCaps};

use crate::policy::Action;

/// The no-prompts values of an agent's mode select, by adapter naming:
/// claude-agent-acp `bypassPermissions`, codex-acp `agent-full-access`,
/// Devin `bypass`, Antigravity `yolo`.
pub(super) const BYPASS_MODES: [&str; 7] = [
    "bypassPermissions",
    "bypass_permissions",
    "bypass",
    "yolo",
    "agent-full-access",
    "danger-full-access",
    "full-access",
];

/// Planning modes (Devin `plan`).
const PLAN_MODES: [&str; 2] = ["plan", "architect"];

/// The modes that ask before writes and commands: Devin `normal`,
/// Antigravity and Hermes `default`. Never an agent's read-only "ask" agent
/// mode (Devin's `ask` answers questions without code changes).
const ASKING_MODES: [&str; 3] = ["default", "normal", "manual"];

/// The agent's mode value for a run under `mode`, among the `available`
/// ones; `None` leaves the agent's current mode alone.
pub(super) fn policy_mode_value(available: &[&str], mode: PermissionMode) -> Option<&'static str> {
    let first = |candidates: &[&'static str]| {
        candidates
            .iter()
            .copied()
            .find(|value| available.contains(value))
    };
    match mode {
        PermissionMode::Bypass => first(&BYPASS_MODES),
        PermissionMode::Plan => first(&PLAN_MODES).or_else(|| first(&ASKING_MODES)),
        PermissionMode::Auto | PermissionMode::AcceptEdits | PermissionMode::Ask => {
            first(&ASKING_MODES)
        }
    }
}

/// Grok picks its permission mode from its own config (`[ui]
/// permission_mode`, often `always-approve`) and offers no mode over ACP,
/// so Zeron can't make it ask: Bypass only, until an OS sandbox confines it.
pub(super) fn grok_policy_caps() -> PolicyCaps {
    PolicyCaps::bypass_only()
}

/// Devin's `normal` mode asks before writes and commands, and it has a
/// native `plan` mode.
pub(super) fn devin_policy_caps() -> PolicyCaps {
    PolicyCaps {
        native_plan: true,
        ..PolicyCaps::all_modes()
    }
}

/// Hermes asks before edits, but before shell commands only when its own
/// detector finds them dangerous: an ordinary command runs unasked in any
/// of its modes. Bypass only.
pub(super) fn hermes_policy_caps() -> PolicyCaps {
    PolicyCaps::bypass_only()
}

/// Antigravity's `default` mode asks before every tool; it has no plan
/// mode (Zeron's policy refuses edits instead).
pub(super) fn antigravity_policy_caps() -> PolicyCaps {
    PolicyCaps::all_modes()
}

/// A `session/request_permission`'s tool call as a policy [`Action`]: the
/// ACP `kind` (read/edit/delete/move/search/execute/think/fetch/
/// switch_mode/other), the `locations`, and the command or URL from
/// `rawInput`.
pub(super) fn permission_action(params: &Value) -> Action {
    let call = params.get("toolCall").unwrap_or(&Value::Null);
    let title = call
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let tool = if title.is_empty() { "tool" } else { title };
    let raw = call.get("rawInput").unwrap_or(&Value::Null);
    let raw_str = |keys: &[&str]| {
        keys.iter().find_map(|key| match raw.get(*key) {
            Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
            Some(Value::Array(words)) if !words.is_empty() => Some(
                words
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
    };
    let mut paths: Vec<PathBuf> = call
        .get("locations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|location| location.get("path").and_then(Value::as_str))
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect();
    if paths.is_empty()
        && let Some(path) = raw_str(&[
            "path",
            "file_path",
            "filePath",
            "abs_path",
            "absolute_path",
            "TargetFile",
            "AbsolutePath",
        ])
    {
        paths.push(PathBuf::from(path));
    }
    let with_paths = |kind: ActionKind| Action {
        paths: paths.clone(),
        ..Action::new(kind, tool)
    };
    let mcp = call
        .get("_meta")
        .and_then(Value::as_object)
        .is_some_and(|meta| meta.keys().any(|key| key.contains("mcp")));
    match call.get("kind").and_then(Value::as_str).unwrap_or("other") {
        "read" | "search" | "think" => with_paths(ActionKind::Read),
        "edit" | "delete" | "move" => with_paths(ActionKind::Edit),
        "execute" => Action::exec(
            tool,
            raw_str(&["command", "CommandLine", "commandLine", "cmd", "script"])
                .unwrap_or_else(|| title.to_owned()),
        ),
        "fetch" => Action {
            host: raw_str(&["url", "Url", "uri"])
                .or_else(|| Some(title.to_owned()))
                .and_then(|url| reqwest::Url::parse(&url).ok())
                .and_then(|url| url.host_str().map(str::to_owned)),
            ..Action::new(ActionKind::Network, tool)
        },
        _ if mcp => Action {
            mcp: Some(tool.to_owned()),
            ..Action::new(ActionKind::Mcp, tool)
        },
        // `switch_mode` (leaving a plan mode) and anything unclassified.
        _ => with_paths(ActionKind::Other),
    }
}

/// Options that would allow more than this one call: a mode switch
/// (Devin's "Yes, switch to bypass", plan exits into accept-edits), an
/// organisation-wide grant. The policy never picks them.
fn widens_permissions(option: &Value) -> bool {
    let text = format!(
        "{} {}",
        option
            .get("optionId")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        option
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
    )
    .to_lowercase();
    [
        "bypass",
        "yolo",
        "full-access",
        "full_access",
        "dangerous",
        "switch",
        "global",
        "accept_edits",
        "accept-edits",
        "accept edits",
        "auto_edit",
        "dont_ask",
        "don't ask",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

fn kind_of(option: &Value) -> &str {
    option
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn option_id(option: &Value) -> Option<String> {
    option
        .get("optionId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// The option that allows exactly this call: an `allow_once`, else an
/// `allow_always` (the policy remembers "always" itself; the agent's own
/// "always" may persist across sessions or switch modes), never one that
/// widens permissions and never a reject. `None` = nothing safe to pick.
pub(super) fn policy_allow_option(options: &[Value]) -> Option<String> {
    ["allow_once", "allow_always"].iter().find_map(|kind| {
        options
            .iter()
            .filter(|option| kind_of(option) == *kind && !widens_permissions(option))
            .find_map(option_id)
    })
}

/// The option that refuses this call only: `reject_once`, else `None` (the
/// caller answers `cancelled` rather than persisting a `reject_always`).
pub(super) fn policy_reject_option(options: &[Value]) -> Option<String> {
    options
        .iter()
        .filter(|option| kind_of(option) == "reject_once")
        .find_map(option_id)
}

/// The `session/request_permission` result selecting `option`, or
/// `cancelled` when there is none.
pub(super) fn permission_outcome(option: Option<String>) -> Value {
    match option {
        Some(option_id) => {
            serde_json::json!({ "outcome": { "outcome": "selected", "optionId": option_id } })
        }
        None => serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn modes_map_per_agent() {
        let devin = [
            "normal",
            "accept-edits",
            "smart",
            "plan",
            "ask",
            "bypass",
            "autonomous",
        ];
        let antigravity = ["default", "auto_edit", "yolo"];
        let hermes = ["default", "accept_edits", "dont_ask"];
        let pick = |available: &[&str], mode| policy_mode_value(available, mode);
        assert_eq!(pick(&devin, PermissionMode::Bypass), Some("bypass"));
        assert_eq!(pick(&devin, PermissionMode::Plan), Some("plan"));
        for mode in [
            PermissionMode::Auto,
            PermissionMode::AcceptEdits,
            PermissionMode::Ask,
        ] {
            assert_eq!(pick(&devin, mode), Some("normal"));
            assert_eq!(pick(&antigravity, mode), Some("default"));
            assert_eq!(pick(&hermes, mode), Some("default"));
        }
        assert_eq!(pick(&antigravity, PermissionMode::Bypass), Some("yolo"));
        assert_eq!(pick(&antigravity, PermissionMode::Plan), Some("default"));
        assert_eq!(pick(&hermes, PermissionMode::Bypass), None);
        assert_eq!(pick(&["bypass"], PermissionMode::Ask), None);
    }

    #[test]
    fn caps_are_honest_per_agent() {
        assert_eq!(grok_policy_caps(), PolicyCaps::bypass_only());
        assert_eq!(hermes_policy_caps(), PolicyCaps::bypass_only());
        let devin = devin_policy_caps();
        assert_eq!(devin.modes, PermissionMode::ALL.to_vec());
        assert!(devin.native_plan);
        let antigravity = antigravity_policy_caps();
        assert_eq!(antigravity.modes, PermissionMode::ALL.to_vec());
        assert!(!antigravity.native_plan);
    }

    #[test]
    fn tool_calls_become_actions() {
        let action = |call: Value| permission_action(&json!({ "toolCall": call }));
        let exec = action(
            json!({"kind":"execute","title":"run_command","rawInput":{"CommandLine":"cargo test"}}),
        );
        assert_eq!(exec.kind, ActionKind::Exec);
        assert_eq!(exec.command.as_deref(), Some("cargo test"));
        let hermes = action(
            json!({"kind":"execute","title":"git push -f","rawInput":{"command":"git push -f","description":"push"}}),
        );
        assert_eq!(hermes.command.as_deref(), Some("git push -f"));
        let argv =
            action(json!({"kind":"execute","title":"x","rawInput":{"command":["git","status"]}}));
        assert_eq!(argv.command.as_deref(), Some("git status"));
        let edit = action(json!({"kind":"edit","title":"Edit","locations":[{"path":"/w/a.rs"}]}));
        assert_eq!(edit.kind, ActionKind::Edit);
        assert_eq!(edit.paths, vec![PathBuf::from("/w/a.rs")]);
        let delete =
            action(json!({"kind":"delete","title":"rm","rawInput":{"file_path":"/w/b.rs"}}));
        assert_eq!(delete.kind, ActionKind::Edit);
        assert_eq!(delete.paths, vec![PathBuf::from("/w/b.rs")]);
        let fetch = action(
            json!({"kind":"fetch","title":"read_url_content","rawInput":{"Url":"https://docs.rs/x"}}),
        );
        assert_eq!(fetch.host.as_deref(), Some("docs.rs"));
        assert_eq!(
            action(json!({"kind":"search","title":"grep"})).kind,
            ActionKind::Read
        );
        assert_eq!(
            action(json!({"kind":"switch_mode","title":"Exit plan"})).kind,
            ActionKind::Other
        );
        let mcp = action(
            json!({"kind":"other","title":"zeron/list_chats","_meta":{"mcp":{"tool":"list_chats"}}}),
        );
        assert_eq!(mcp.kind, ActionKind::Mcp);
    }

    #[test]
    fn the_policy_never_picks_a_reject_or_a_widening_option_as_allow() {
        // Devin's exec prompt.
        let devin = vec![
            json!({"optionId":"switch_bypass","name":"Yes, switch to bypass mode","kind":"allow_always"}),
            json!({"optionId":"allow_always_global","name":"Always (all projects)","kind":"allow_always"}),
            json!({"optionId":"allow_session","name":"Allow for session","kind":"allow_always"}),
            json!({"optionId":"allow_once","name":"Allow","kind":"allow_once"}),
            json!({"optionId":"reject_once","name":"Reject","kind":"reject_once"}),
        ];
        assert_eq!(policy_allow_option(&devin).as_deref(), Some("allow_once"));
        assert_eq!(policy_reject_option(&devin).as_deref(), Some("reject_once"));
        // Devin's plan exit: every allow switches modes.
        let plan_exit = vec![
            json!({"optionId":"plan_bypass","name":"Yes, implement plan and bypass permissions","kind":"allow_once"}),
            json!({"optionId":"plan_accept_edits","name":"Yes, implement plan and accept edits","kind":"allow_once"}),
            json!({"optionId":"plan_normal","name":"Yes, implement plan","kind":"allow_once"}),
            json!({"optionId":"keep_planning","name":"No, plan needs changes","kind":"reject_once"}),
        ];
        assert_eq!(
            policy_allow_option(&plan_exit).as_deref(),
            Some("plan_normal")
        );
        // Only rejects (or only widening allows): nothing to allow with.
        let rejects = vec![
            json!({"optionId":"no","name":"No","kind":"reject_once"}),
            json!({"optionId":"never","name":"Never","kind":"reject_always"}),
        ];
        assert_eq!(policy_allow_option(&rejects), None);
        assert_eq!(policy_allow_option(&devin[..2]), None);
        // An always-only agent still gets an allow, never the reject.
        let always_only = vec![
            json!({"optionId":"deny","name":"Deny","kind":"reject_once"}),
            json!({"optionId":"ok","name":"Allow Always","kind":"allow_always"}),
        ];
        assert_eq!(policy_allow_option(&always_only).as_deref(), Some("ok"));
        assert_eq!(policy_reject_option(&rejects[1..]), None);
        assert_eq!(
            permission_outcome(None),
            json!({"outcome":{"outcome":"cancelled"}})
        );
    }
}
