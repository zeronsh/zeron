//! Zeron's permission modes on Codex
//! (docs/plans/2026-09-30-agent-mobility-and-policy.md, Part 4).
//!
//! Codex enforces its own approval policy and sandbox; we choose both from
//! the run's policy and answer the approval requests it sends through the
//! shared [`Gate`]:
//!
//! | Zeron mode              | `approvalPolicy` | sandbox                                      |
//! |-------------------------|------------------|----------------------------------------------|
//! | Bypass                  | `never`          | Off → `danger-full-access` (as before), else the chosen sandbox |
//! | Auto, AcceptEdits, Ask  | `untrusted`      | Off → `danger-full-access`, else the chosen sandbox |
//! | Plan                    | `untrusted`      | `read-only`                                  |
//!
//! `untrusted` (codex-cli 0.159: `untrusted` / `on-request` / `never` or a
//! granular object) asks before every command outside Codex's own known-safe
//! read-only set and before every patch, so the gate sees each edit and
//! command; `on-request` would let sandboxed commands run unasked.
//! Workspace-write's network access follows `policy.network`.
//!
//! Both ride every `turn/start`, but a different mode still arrives as a new
//! run: steers and follow-up turns carry no policy, and the engine restarts a
//! runtime whose configuration changed.
//!
//! Codex has an experimental native plan mode (`turn/start`'s
//! `collaborationMode: {mode: "plan", settings: {model, …}}`, listed by
//! `collaborationMode/list`), which streams its plan as `plan` items; it is
//! not driven here — Plan is Zeron's read-only sandbox plus the gate.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use zeron_proto::{ActionKind, AgentPolicy, PermissionMode, SandboxLevel, SandboxMode};

use crate::policy::{Action, real_path};

/// What Codex is told for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Wire {
    pub approval_policy: &'static str,
    pub sandbox: SandboxLevel,
    /// Network access inside a workspace-write or read-only sandbox.
    pub network: bool,
}

pub(crate) fn wire(policy: &AgentPolicy, title_only: bool) -> Wire {
    if title_only {
        return Wire {
            approval_policy: "never",
            sandbox: SandboxLevel::ReadOnly,
            network: false,
        };
    }
    let approval_policy = match policy.mode {
        PermissionMode::Bypass => "never",
        _ => "untrusted",
    };
    let sandbox = match (policy.mode, policy.sandbox) {
        (PermissionMode::Plan, _) | (_, SandboxMode::ReadOnly) => SandboxLevel::ReadOnly,
        (_, SandboxMode::WorkspaceWrite) => SandboxLevel::WorkspaceWrite,
        (_, SandboxMode::Off) => SandboxLevel::DangerFullAccess,
    };
    Wire {
        approval_policy,
        sandbox,
        network: policy.network,
    }
}

pub(crate) const COMMAND_APPROVAL: &str = "item/commandExecution/requestApproval";
pub(crate) const FILE_CHANGE_APPROVAL: &str = "item/fileChange/requestApproval";

/// The paths of a `fileChange` item (`changes[].path`).
pub(crate) fn change_paths(item: &Value) -> Vec<PathBuf> {
    item.get("changes")
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|c| c.get("path").and_then(Value::as_str))
        .map(|path| real_path(Path::new(path)))
        .collect()
}

/// What an approval request asks to do. File-change requests name only the
/// item: its paths come from the `fileChange` item's `item/started`, which
/// the app server sends first. A patch whose files are unknown is `Other`,
/// so no mode waves it through as a project edit.
pub(crate) fn action_for(
    method: &str,
    params: &Value,
    file_changes: &HashMap<String, Vec<PathBuf>>,
) -> Action {
    if method == COMMAND_APPROVAL {
        if let Some(host) = params
            .pointer("/networkApprovalContext/host")
            .and_then(Value::as_str)
        {
            return Action {
                host: Some(host.to_owned()),
                ..Action::new(ActionKind::Network, "shell")
            };
        }
        let command = match params.get("command") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(parts)) => parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        };
        return Action::exec("shell", command);
    }
    let mut paths = change_paths(params);
    if paths.is_empty()
        && let Some(known) = params
            .get("itemId")
            .and_then(Value::as_str)
            .and_then(|id| file_changes.get(id))
    {
        paths = known.clone();
    }
    if let Some(root) = params.get("grantRoot").and_then(Value::as_str) {
        paths.push(real_path(Path::new(root)));
    }
    if paths.is_empty() {
        return Action::new(ActionKind::Other, "apply_patch");
    }
    Action {
        paths,
        ..Action::new(ActionKind::Edit, "apply_patch")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bypass_keeps_yolo_and_the_rest_ask_through_the_gate() {
        let w = |mode, sandbox| {
            wire(
                &AgentPolicy {
                    mode,
                    sandbox,
                    ..AgentPolicy::default()
                },
                false,
            )
        };
        let bypass = w(PermissionMode::Bypass, SandboxMode::Off);
        assert_eq!(bypass.approval_policy, "never");
        assert_eq!(bypass.sandbox, SandboxLevel::DangerFullAccess);
        for mode in [
            PermissionMode::Auto,
            PermissionMode::AcceptEdits,
            PermissionMode::Ask,
        ] {
            assert_eq!(w(mode, SandboxMode::Off).approval_policy, "untrusted");
            assert_eq!(
                w(mode, SandboxMode::Off).sandbox,
                SandboxLevel::DangerFullAccess
            );
            assert_eq!(
                w(mode, SandboxMode::WorkspaceWrite).sandbox,
                SandboxLevel::WorkspaceWrite
            );
            assert_eq!(
                w(mode, SandboxMode::ReadOnly).sandbox,
                SandboxLevel::ReadOnly
            );
        }
        let plan = w(PermissionMode::Plan, SandboxMode::Off);
        assert_eq!(
            (plan.approval_policy, plan.sandbox),
            ("untrusted", SandboxLevel::ReadOnly)
        );
        let title = wire(&AgentPolicy::with_mode(PermissionMode::Ask), true);
        assert_eq!(
            (title.approval_policy, title.sandbox),
            ("never", SandboxLevel::ReadOnly)
        );
    }

    #[test]
    fn approval_requests_become_actions() {
        let none = HashMap::new();
        let exec = action_for(
            COMMAND_APPROVAL,
            &json!({"itemId": "c1", "command": "cargo test"}),
            &none,
        );
        assert_eq!(exec.kind, ActionKind::Exec);
        assert_eq!(exec.command.as_deref(), Some("cargo test"));
        let net = action_for(
            COMMAND_APPROVAL,
            &json!({"itemId": "c1", "command": "curl x", "networkApprovalContext": {"host": "x.dev", "protocol": "https"}}),
            &none,
        );
        assert_eq!(
            (net.kind, net.host.as_deref()),
            (ActionKind::Network, Some("x.dev"))
        );

        let known = HashMap::from([("f1".to_owned(), vec![PathBuf::from("/w/a.rs")])]);
        let edit = action_for(FILE_CHANGE_APPROVAL, &json!({"itemId": "f1"}), &known);
        assert_eq!(edit.kind, ActionKind::Edit);
        assert_eq!(edit.paths, vec![PathBuf::from("/w/a.rs")]);
        let unknown = action_for(FILE_CHANGE_APPROVAL, &json!({"itemId": "f9"}), &known);
        assert_eq!(unknown.kind, ActionKind::Other);
        let root = action_for(
            FILE_CHANGE_APPROVAL,
            &json!({"itemId": "f1", "grantRoot": "/zz-root"}),
            &known,
        );
        assert_eq!(
            root.paths,
            vec![PathBuf::from("/w/a.rs"), PathBuf::from("/zz-root")]
        );
    }
}
