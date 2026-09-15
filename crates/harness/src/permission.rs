//! Shared Permission trait: the approval setting every harness exposes in
//! the model picker's traits tray.
//!
//! Native drivers honor harness-specific choice ids (Claude permission-mode,
//! Codex approvalPolicy, OpenCode reply). ACP agents that advertise a `mode`
//! select reuse that wire (Droid's autonomy_level, Codex-style
//! agent-full-access). Agents that don't get a client-side Ask / Auto (High)
//! pair applied to `session/request_permission`.

use serde_json::{Map, Value};

use zeron_proto::{Model, ModelOption, ModelOptionChoice};

pub const ID: &str = "permission";

/// Values that mean "don't stop for permission prompts".
pub const AUTO_VALUES: &[&str] = &[
    "bypassPermissions",
    "bypass_permissions",
    "bypass",
    "yolo",
    "agent-full-access",
    "danger-full-access",
    "full-access",
    "auto-high",
    "auto_high",
    "never",
    "always",
];

pub fn selected(options: &Map<String, Value>) -> Option<&str> {
    ["permission", "autonomy_level", "mode"]
        .into_iter()
        .find_map(|key| {
            options
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
}

/// Default is unattended: a missing pick is Auto (High).
pub fn auto_allows(options: &Map<String, Value>) -> bool {
    match selected(options) {
        None => true,
        Some(value) => AUTO_VALUES.iter().any(|v| *v == value),
    }
}

pub fn option(choices: &[(&str, &str)], default: &str) -> ModelOption {
    ModelOption {
        id: ID.into(),
        label: "Permission".into(),
        choices: choices
            .iter()
            .map(|(id, label)| ModelOptionChoice {
                id: (*id).into(),
                label: (*label).into(),
            })
            .collect(),
        default_choice: default.into(),
    }
}

pub fn claude() -> ModelOption {
    option(
        &[
            ("default", "Ask"),
            ("acceptEdits", "Auto (edits)"),
            ("plan", "Plan"),
            ("bypassPermissions", "Auto (High)"),
        ],
        "bypassPermissions",
    )
}

pub fn codex() -> ModelOption {
    option(
        &[
            ("on-request", "Ask"),
            ("on-failure", "Auto (on failure)"),
            ("untrusted", "Sandbox"),
            ("never", "Auto (High)"),
        ],
        "never",
    )
}

pub fn opencode() -> ModelOption {
    option(&[("once", "Ask"), ("always", "Auto (High)")], "always")
}

/// Client-side Ask / Auto for ACP agents that don't advertise a mode select,
/// and for Cursor (the SDK has no approval surface).
pub fn client() -> ModelOption {
    option(&[("ask", "Ask"), ("auto-high", "Auto (High)")], "auto-high")
}

pub fn droid() -> ModelOption {
    option(
        &[
            ("normal", "Auto (Off)"),
            ("spec", "Spec"),
            ("auto-low", "Auto (Low)"),
            ("auto-medium", "Auto (Medium)"),
            ("auto-high", "Auto (High)"),
        ],
        "auto-high",
    )
}

pub fn has_option(model: &Model) -> bool {
    model.options.iter().any(|o| {
        o.id == ID
            || o.id == "autonomy_level"
            || o.id == "autonomyLevel"
            || o.id == "mode"
            || o.label.eq_ignore_ascii_case("permission")
    })
}

pub fn prepend(model: &mut Model, extra: ModelOption) {
    if !has_option(model) {
        model.options.insert(0, extra);
    }
}

pub fn with(mut options: Vec<ModelOption>, extra: ModelOption) -> Vec<ModelOption> {
    if !options.iter().any(|o| {
        o.id == extra.id || o.id == ID || o.id == "autonomy_level" || o.id == "mode"
    }) {
        options.insert(0, extra);
    }
    options
}

pub fn ensure_all(models: &mut [Model], extra: ModelOption) {
    for model in models {
        prepend(model, extra.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_pick_is_unattended() {
        assert!(auto_allows(&Map::new()));
    }

    #[test]
    fn ask_does_not_auto_allow() {
        let mut opts = Map::new();
        opts.insert("permission".into(), Value::String("default".into()));
        assert!(!auto_allows(&opts));
        opts.insert("permission".into(), Value::String("on-request".into()));
        assert!(!auto_allows(&opts));
        opts.insert("autonomy_level".into(), Value::String("auto-low".into()));
        assert!(!auto_allows(&opts));
    }

    #[test]
    fn auto_high_aliases_auto_allow() {
        for value in ["bypassPermissions", "never", "always", "auto-high"] {
            let mut opts = Map::new();
            opts.insert("permission".into(), Value::String(value.into()));
            assert!(auto_allows(&opts), "{value}");
        }
    }
}
