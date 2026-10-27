//! Approvals asked of the person. A run that is not in full access asks
//! before what its [`AccessMode`] does not cover; every driver asks the same
//! way — a yes/no question through [`crate::RunControls::request_input`] —
//! so an approval reads alike whichever agent raised it.
use zeron_proto::{AccessMode, UserInputAnswer, UserInputQuestion};

pub const ALLOW: &str = "Yes";
pub const DENY: &str = "No";

/// What an agent wants to do, for deciding whether its mode covers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Write, edit or delete files.
    Edit,
    /// Anything else: a shell command, network, an MCP or unknown tool.
    Other,
}

/// Whether `access` lets the agent go ahead with `action` without asking.
pub fn allowed_without_asking(access: AccessMode, action: Action) -> bool {
    match access {
        AccessMode::FullAccess => true,
        AccessMode::AutoAcceptEdits => action == Action::Edit,
        AccessMode::Supervised => false,
    }
}

/// A yes/no approval question: `header` names the kind ("Approve command"),
/// `question` says what and ends asking ("… Allow it?").
pub fn question(header: impl Into<String>, question: impl Into<String>) -> UserInputQuestion {
    UserInputQuestion {
        id: uuid::Uuid::new_v4().to_string(),
        header: header.into(),
        question: question.into(),
        options: vec![ALLOW.into(), DENY.into()],
        multi_select: false,
        prefill: None,
        multiline: false,
    }
}

/// Whether `answers` approve `question`. Anything but an explicit yes —
/// a no, free text, or the questions closed unanswered — is a refusal, so
/// the agent is never allowed silently.
pub fn approved(question: &UserInputQuestion, answers: &[UserInputAnswer]) -> bool {
    answers.iter().any(|a| {
        a.question_id == question.id && a.labels.iter().any(|l| l.eq_ignore_ascii_case(ALLOW))
    })
}

/// "`agent` wants to run `command`. Allow it?"
pub fn command(agent: &str, command: &str) -> UserInputQuestion {
    let command = command.trim();
    question(
        "Approve command",
        if command.is_empty() {
            format!("{agent} wants to run a command. Allow it?")
        } else {
            format!("{agent} wants to run `{command}`. Allow it?")
        },
    )
}

/// "`agent` wants to modify a, b. Allow it?"
pub fn edit(agent: &str, paths: &[&str]) -> UserInputQuestion {
    question(
        "Approve file change",
        if paths.is_empty() {
            format!("{agent} wants to modify files. Allow it?")
        } else {
            format!("{agent} wants to modify {}. Allow it?", paths.join(", "))
        },
    )
}

/// "`agent` wants to use `tool`. Allow it?", with what it says it is for.
pub fn tool(agent: &str, tool: &str, detail: Option<&str>) -> UserInputQuestion {
    let detail = detail
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| format!(" ({d})"))
        .unwrap_or_default();
    question(
        "Approve tool",
        format!("{agent} wants to use {tool}{detail}. Allow it?"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_cover_what_they_say() {
        use AccessMode::*;
        assert!(allowed_without_asking(FullAccess, Action::Other));
        assert!(allowed_without_asking(AutoAcceptEdits, Action::Edit));
        assert!(!allowed_without_asking(AutoAcceptEdits, Action::Other));
        assert!(!allowed_without_asking(Supervised, Action::Edit));
    }

    #[test]
    fn only_an_explicit_yes_approves() {
        let q = command("Muse", "cargo test");
        assert_eq!(q.question, "Muse wants to run `cargo test`. Allow it?");
        let answer = |labels: &[&str]| {
            vec![UserInputAnswer {
                question_id: q.id.clone(),
                labels: labels.iter().map(|l| l.to_string()).collect(),
            }]
        };
        assert!(approved(&q, &answer(&["Yes"])));
        assert!(!approved(&q, &answer(&["No"])));
        assert!(!approved(&q, &answer(&["sure, go ahead"])));
        assert!(!approved(&q, &[]));
    }
}
