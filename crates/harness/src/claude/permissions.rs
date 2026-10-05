//! Zeron's permission modes on Claude Code
//! (docs/plans/2026-09-30-agent-mobility-and-policy.md, Part 4).
//!
//! The CLI keeps its own permission engine; we pick its mode at launch and
//! answer every `can_use_tool` it can't settle itself through the shared
//! [`Gate`]:
//!
//! | Zeron mode    | `--permission-mode`                                   | `can_use_tool`        |
//! |---------------|-------------------------------------------------------|-----------------------|
//! | Bypass        | `bypassPermissions` + `--dangerously-skip-permissions` when `auto_approve`, else `default` | allowed (deny rules still hold) |
//! | Auto, Ask     | `default`                                             | gate                  |
//! | AcceptEdits   | `acceptEdits` (the CLI accepts project edits itself)  | gate                  |
//! | Plan          | `plan` (the CLI's native plan mode)                   | gate, Plan semantics  |
//!
//! `default` is the CLI's long-standing name for what 2.1.285's `--help`
//! lists as `manual`; the SDK control schema still spells it `default` and the
//! flag still accepts it.
//!
//! The mode is fixed for a run: a different mode arrives as a new run (the
//! engine restarts a runtime whose configuration changed). The CLI could
//! switch live (`set_permission_mode` control request), but steers carry no
//! policy, so nothing would trigger it.

use std::path::Path;

use serde_json::{Value, json};
use zeron_proto::{ActionKind, PermissionMode, RunRequest};

use crate::policy::{Action, Decision, Gate, real_path};

/// `--permission-mode …` (plus the skip flag) for `request`.
pub(crate) fn permission_args(request: &RunRequest) -> &'static [&'static str] {
    match request.policy.mode {
        PermissionMode::Bypass if request.auto_approve => &[
            "--permission-mode",
            "bypassPermissions",
            "--dangerously-skip-permissions",
        ],
        PermissionMode::Bypass | PermissionMode::Auto | PermissionMode::Ask => {
            &["--permission-mode", "default"]
        }
        PermissionMode::AcceptEdits => &["--permission-mode", "acceptEdits"],
        PermissionMode::Plan => &["--permission-mode", "plan"],
    }
}

/// The tool the CLI calls to leave its plan mode; its input carries the plan.
pub(crate) const EXIT_PLAN_MODE: &str = "ExitPlanMode";

/// What a `can_use_tool` request asks to do.
pub(crate) fn action_for(tool: &str, input: &Value) -> Action {
    let field = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_owned);
    let with_path = |kind: ActionKind, key: &str| match field(key) {
        Some(path) => Action::path(kind, tool, real_path(Path::new(&path))),
        None => Action::new(kind, tool),
    };
    match tool {
        "Bash" | "PowerShell" => Action::exec(tool, field("command").unwrap_or_default()),
        "Edit" | "MultiEdit" | "Write" => with_path(ActionKind::Edit, "file_path"),
        "NotebookEdit" => with_path(ActionKind::Edit, "notebook_path"),
        "Read" => with_path(ActionKind::Read, "file_path"),
        "NotebookRead" => with_path(ActionKind::Read, "notebook_path"),
        "Glob" | "Grep" | "LS" => with_path(ActionKind::Read, "path"),
        "WebFetch" => Action {
            host: field("url").as_deref().and_then(url_host),
            ..Action::new(ActionKind::Network, tool)
        },
        "WebSearch" => Action::new(ActionKind::Network, tool),
        _ => match tool.strip_prefix("mcp__") {
            Some(rest) => Action {
                mcp: Some(rest.to_owned()),
                ..Action::new(ActionKind::Mcp, tool)
            },
            None => Action::new(ActionKind::Other, tool),
        },
    }
}

/// `https://user@Example.com:8080/x?y` → `example.com`.
fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = if host.starts_with('[') {
        host.split(']').next().map(|h| format!("{h}]"))?
    } else {
        host.split(':').next()?.to_owned()
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// The gate's verdict on a `can_use_tool` request. Leaving the native plan
/// mode is the user's call (Plan semantics would refuse it as not read-only).
pub(crate) fn verdict(gate: &Gate, action: &Action, exit_plan: bool) -> Decision {
    if exit_plan && gate.policy.mode == PermissionMode::Plan {
        Decision::Ask
    } else {
        gate.decide(action)
    }
}

/// `can_use_tool` refusal; the message reaches the model as the tool result.
pub(crate) fn deny_response(message: &str) -> Value {
    json!({ "behavior": "deny", "message": message })
}

/// Approving `ExitPlanMode`: the CLI leaves plan mode for its `default` mode,
/// said explicitly so the CLI and the gate (now `Ask`) agree.
pub(crate) fn exit_plan_allow_response(input: Value) -> Value {
    json!({
        "behavior": "allow",
        "updatedInput": input,
        "updatedPermissions": [
            { "type": "setMode", "mode": "default", "destination": "session" }
        ],
    })
}

pub(crate) const USER_DENIED: &str = "The user denied this action.";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn request(mode: PermissionMode, auto_approve: bool) -> RunRequest {
        let mut request: RunRequest = serde_json::from_value(json!({
            "prompt": "x", "cwd": "", "sandbox": "danger-full-access", "autoApprove": auto_approve,
        }))
        .unwrap();
        request.policy.mode = mode;
        request
    }

    #[test]
    fn bypass_keeps_the_old_flags() {
        assert_eq!(
            permission_args(&request(PermissionMode::Bypass, true)),
            [
                "--permission-mode",
                "bypassPermissions",
                "--dangerously-skip-permissions"
            ]
        );
        assert_eq!(
            permission_args(&request(PermissionMode::Bypass, false)),
            ["--permission-mode", "default"]
        );
    }

    #[test]
    fn modes_map_to_cli_modes() {
        let mode = |m| permission_args(&request(m, true))[1];
        assert_eq!(mode(PermissionMode::Auto), "default");
        assert_eq!(mode(PermissionMode::Ask), "default");
        assert_eq!(mode(PermissionMode::AcceptEdits), "acceptEdits");
        assert_eq!(mode(PermissionMode::Plan), "plan");
        for m in [
            PermissionMode::Auto,
            PermissionMode::Ask,
            PermissionMode::AcceptEdits,
            PermissionMode::Plan,
        ] {
            assert!(
                !permission_args(&request(m, true)).contains(&"--dangerously-skip-permissions")
            );
        }
    }

    #[test]
    fn tool_requests_become_actions() {
        let bash = action_for(
            "Bash",
            &json!({"command": "cargo test", "description": "t"}),
        );
        assert_eq!(bash.kind, ActionKind::Exec);
        assert_eq!(bash.command.as_deref(), Some("cargo test"));
        let edit = action_for(
            "MultiEdit",
            &json!({"file_path": "/w/src/a.rs", "edits": []}),
        );
        assert_eq!(edit.kind, ActionKind::Edit);
        assert_eq!(edit.paths, vec![PathBuf::from("/w/src/a.rs")]);
        let nb = action_for("NotebookEdit", &json!({"notebook_path": "/w/a.ipynb"}));
        assert_eq!(nb.paths, vec![PathBuf::from("/w/a.ipynb")]);
        assert_eq!(
            action_for("Grep", &json!({"pattern": "x"})).kind,
            ActionKind::Read
        );
        let fetch = action_for("WebFetch", &json!({"url": "https://Docs.rs:443/tokio?x=1"}));
        assert_eq!(fetch.kind, ActionKind::Network);
        assert_eq!(fetch.host.as_deref(), Some("docs.rs"));
        let mcp = action_for("mcp__linear__search", &json!({}));
        assert_eq!(mcp.kind, ActionKind::Mcp);
        assert_eq!(mcp.mcp.as_deref(), Some("linear__search"));
        assert_eq!(action_for("Agent", &json!({})).kind, ActionKind::Other);
        assert_eq!(
            action_for(EXIT_PLAN_MODE, &json!({"plan": "p"})).kind,
            ActionKind::Other
        );
    }

    #[test]
    fn hosts() {
        assert_eq!(url_host("http://a@b.c/x").as_deref(), Some("b.c"));
        assert_eq!(url_host("example.com/x").as_deref(), Some("example.com"));
        assert_eq!(url_host("http://[::1]:80/").as_deref(), Some("[::1]"));
        assert_eq!(url_host("https:///x"), None);
    }
}
