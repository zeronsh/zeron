//! Tool permissions under a run's [`AccessMode`]. Full access answers every
//! `session/request_permission` with the agent's preferred allow option, as
//! ACP runs always have. Any other mode goes by the request's
//! `toolCall.kind`: looking around (read, search, think) is always allowed,
//! file changes (edit, delete, move) are allowed under auto-accept edits,
//! and the rest — commands, fetches, an unknown or missing kind — is asked
//! of the person. Yes allows once, no rejects once, and a permission no one
//! can be asked about (setup, a control call) is refused.
use serde_json::{Value, json};
use zeron_proto::{AccessMode, ToolCall, UserInputQuestion};

use crate::approval::{self, Action};

/// What a permission asks for, from its tool call's `kind`; `None` for
/// looking around, which no mode asks about.
fn action(params: &Value) -> Option<Action> {
    match params["toolCall"]["kind"].as_str() {
        Some("read" | "search" | "think") => None,
        Some("edit" | "delete" | "move") => Some(Action::Edit),
        _ => Some(Action::Other),
    }
}

/// Whether `access` lets the permission in `params` through unasked.
pub(super) fn allowed_without_asking(access: AccessMode, params: &Value) -> bool {
    action(params).is_none_or(|action| approval::allowed_without_asking(access, action))
}

/// The answer to a permission: yes selects `allow_once` — never
/// `allow_always`, which would stop the agent asking — and no `reject_once`
/// (else `reject_always`). Without such an option the request is
/// `cancelled`, never answered with another one.
pub(super) fn outcome(options: &[Value], allow: bool) -> Value {
    let kinds: &[&str] = if allow {
        &["allow_once"]
    } else {
        &["reject_once", "reject_always"]
    };
    let option = kinds
        .iter()
        .find_map(|kind| options.iter().find(|o| o["kind"] == *kind))
        .and_then(|o| o["optionId"].as_str());
    match option {
        Some(id) => json!({ "outcome": { "outcome": "selected", "optionId": id } }),
        None => json!({ "outcome": { "outcome": "cancelled" } }),
    }
}

/// What a permission asks the person: the command or the files when the
/// tool call carries them, else the tool's title.
pub(super) fn question(agent: &str, params: &Value) -> UserInputQuestion {
    let tool = &params["toolCall"];
    match super::normalize::typed_call(tool) {
        ToolCall::Exec { command } if !command.is_empty() => approval::command(agent, &command),
        ToolCall::EditFile { path, .. } | ToolCall::WriteFile { path, .. } => {
            approval::edit(agent, &[&path])
        }
        ToolCall::ApplyPatch { path } => approval::edit(agent, &Vec::from_iter(path.as_deref())),
        _ => {
            let title = tool["title"].as_str().map(str::trim).unwrap_or_default();
            let kind = tool["kind"].as_str().unwrap_or_default();
            match (title, action(params)) {
                ("", Some(Action::Edit)) => approval::edit(agent, &[]),
                ("", _) if kind == "execute" => approval::command(agent, ""),
                ("", _) => approval::tool(agent, "a tool", None),
                (title, _) => approval::tool(agent, title, None),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permission(tool: Value) -> Value {
        json!({ "sessionId": "s", "toolCall": tool })
    }

    #[test]
    fn modes_ask_by_tool_kind() {
        use AccessMode::*;
        let kind = |kind: &str| permission(json!({ "toolCallId": "t", "kind": kind }));
        for harmless in ["read", "search", "think"] {
            assert!(
                allowed_without_asking(Supervised, &kind(harmless)),
                "{harmless}"
            );
        }
        for edit in ["edit", "delete", "move"] {
            assert!(!allowed_without_asking(Supervised, &kind(edit)), "{edit}");
            assert!(
                allowed_without_asking(AutoAcceptEdits, &kind(edit)),
                "{edit}"
            );
        }
        for other in ["execute", "fetch", "other", "switch_mode"] {
            assert!(
                !allowed_without_asking(AutoAcceptEdits, &kind(other)),
                "{other}"
            );
            assert!(allowed_without_asking(FullAccess, &kind(other)), "{other}");
        }
        // No kind at all asks like a command would.
        let kindless = permission(json!({ "toolCallId": "t" }));
        assert!(!allowed_without_asking(AutoAcceptEdits, &kindless));
    }

    #[test]
    fn answers_pick_the_once_options_and_never_allow_always() {
        let options = vec![
            json!({ "optionId": "always", "kind": "allow_always" }),
            json!({ "optionId": "once", "kind": "allow_once" }),
            json!({ "optionId": "never", "kind": "reject_always" }),
            json!({ "optionId": "no", "kind": "reject_once" }),
        ];
        let picked =
            |options: &[Value], allow| outcome(options, allow)["outcome"]["optionId"].clone();
        assert_eq!(picked(&options, true), "once");
        assert_eq!(picked(&options, false), "no");
        // Only a lasting allow on offer: a yes must not grant it.
        assert_eq!(
            outcome(&options[..1], true),
            json!({ "outcome": { "outcome": "cancelled" } })
        );
        assert_eq!(picked(&options[..3], false), "never");
        // Nothing to refuse with: cancelled, not an allow.
        assert_eq!(
            outcome(&options[..2], false),
            json!({ "outcome": { "outcome": "cancelled" } })
        );
    }

    #[test]
    fn questions_name_the_command_or_the_files() {
        let ask = |tool| question("Antigravity", &permission(tool)).question;
        assert_eq!(
            ask(
                json!({ "kind": "execute", "title": "Run", "rawInput": { "command": "cargo test" } })
            ),
            "Antigravity wants to run `cargo test`. Allow it?"
        );
        assert_eq!(
            ask(json!({ "kind": "edit", "locations": [{ "path": "/w/src/lib.rs" }] })),
            "Antigravity wants to modify /w/src/lib.rs. Allow it?"
        );
        assert_eq!(
            ask(json!({ "kind": "delete" })),
            "Antigravity wants to modify files. Allow it?"
        );
        assert_eq!(
            ask(json!({ "kind": "fetch", "title": "Fetch https://example.com" })),
            "Antigravity wants to use Fetch https://example.com. Allow it?"
        );
        let asked = question("Antigravity", &permission(json!({ "kind": "execute" })));
        assert_eq!(asked.options, ["Yes", "No"]);
    }
}
