//! Questions an agent asks the person without stopping (Zeron's MCP
//! `ask_question`), and the messages the engine delivers back into the
//! asking chat when they are answered.
//!
//! A question is a registry row (kind `questions`), so every device of the
//! person shows it and any of them can answer. It never changes the chat's
//! session status: unlike a harness's own question (`awaitingInput`), the
//! agent keeps working while it waits, and the answer reaches it later as a
//! message in its chat, steered into a running turn or starting a new one.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Longest question text, in characters. A registry row is an index entry
/// with a 16 KB op budget, never a document.
pub const QUESTION_MAX_CHARS: usize = 6_000;
/// Longest context text, in characters.
pub const QUESTION_CONTEXT_MAX_CHARS: usize = 3_000;
/// At most this many options, each at most [`QUESTION_OPTION_MAX_CHARS`].
pub const QUESTION_MAX_OPTIONS: usize = 12;
pub const QUESTION_OPTION_MAX_CHARS: usize = 200;
/// Longest answer, in characters.
pub const ANSWER_MAX_CHARS: usize = 4_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum QuestionStatus {
    /// Open: shown to the person, waiting for an answer.
    #[default]
    Pending,
    /// The person answered it.
    Answered,
    /// The person closed it without answering. The agent is told.
    Dismissed,
    /// The agent withdrew it.
    Cancelled,
}

impl QuestionStatus {
    pub fn is_open(self) -> bool {
        self == Self::Pending
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentQuestion {
    pub id: String,
    /// The chat whose agent asked, and where the answer is delivered.
    pub chat_id: String,
    /// Markdown.
    pub question: String,
    /// Choices to offer as buttons; the person may still answer in words.
    #[serde(default)]
    pub options: Vec<String>,
    /// Markdown shown under the question, muted.
    #[serde(default)]
    pub context: Option<String>,
    /// The agent is waiting for it inside its tool call: the answer is
    /// returned there, not delivered as a message.
    #[serde(default)]
    pub blocking: bool,
    #[serde(default)]
    pub status: QuestionStatus,
    #[serde(default)]
    pub answer: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub resolved_at: Option<DateTime<Utc>>,
    /// Device that answered or dismissed it.
    #[serde(default)]
    pub resolved_by: Option<String>,
    /// When the answer (or dismissal) went into the asking chat as a message.
    #[serde(default)]
    pub delivered_at: Option<DateTime<Utc>>,
}

/// One line of a question as a delivered message quotes it: its first line,
/// at most 120 characters.
pub fn quote(question: &str) -> String {
    let line = question
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    let mut out: String = line.chars().take(120).collect();
    if line.chars().count() > 120 || question.trim().lines().count() > 1 {
        out.push('…');
    }
    out
}

/// The message an answer arrives as in the asking chat.
pub fn answer_message(question: &AgentQuestion, answer: &str) -> String {
    format!(
        "Answer to your question «{}» (question {}): {}",
        quote(&question.question),
        question.id,
        answer.trim()
    )
}

/// The message a dismissal arrives as in the asking chat.
pub fn dismissed_message(question: &AgentQuestion) -> String {
    format!(
        "The person dismissed your question «{}» (question {}) without answering. Carry on with your best judgment, or ask again differently if you can't.",
        quote(&question.question),
        question.id
    )
}

/// Trim and bound what an agent asks, or say what's wrong.
pub fn validate_question(
    question: &str,
    options: &[String],
    context: Option<&str>,
) -> Result<(String, Vec<String>, Option<String>), String> {
    let question = question.trim();
    if question.is_empty() {
        return Err("question is empty".into());
    }
    if question.chars().count() > QUESTION_MAX_CHARS {
        return Err(format!(
            "question is longer than {QUESTION_MAX_CHARS} characters"
        ));
    }
    if options.len() > QUESTION_MAX_OPTIONS {
        return Err(format!("at most {QUESTION_MAX_OPTIONS} options"));
    }
    let mut cleaned: Vec<String> = Vec::with_capacity(options.len());
    for option in options {
        let option = option.trim();
        if option.is_empty() {
            return Err("an option is empty".into());
        }
        if option.chars().count() > QUESTION_OPTION_MAX_CHARS {
            return Err(format!(
                "an option is longer than {QUESTION_OPTION_MAX_CHARS} characters"
            ));
        }
        if !cleaned.iter().any(|o| o == option) {
            cleaned.push(option.to_owned());
        }
    }
    let context = context.map(str::trim).filter(|c| !c.is_empty());
    if context.is_some_and(|c| c.chars().count() > QUESTION_CONTEXT_MAX_CHARS) {
        return Err(format!(
            "context is longer than {QUESTION_CONTEXT_MAX_CHARS} characters"
        ));
    }
    Ok((question.to_owned(), cleaned, context.map(str::to_owned)))
}

/// Trim and bound an answer, or say what's wrong.
pub fn validate_answer(answer: &str) -> Result<String, String> {
    let answer = answer.trim();
    if answer.is_empty() {
        return Err("answer is empty".into());
    }
    if answer.chars().count() > ANSWER_MAX_CHARS {
        return Err(format!(
            "answer is longer than {ANSWER_MAX_CHARS} characters"
        ));
    }
    Ok(answer.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_takes_the_first_line_and_marks_more() {
        assert_eq!(quote("Ship it?"), "Ship it?");
        assert_eq!(quote("\n  Which branch?\nmore detail"), "Which branch?…");
        assert_eq!(quote(&"x".repeat(130)).chars().count(), 121);
    }

    #[test]
    fn validation_trims_dedupes_and_bounds() {
        let (q, o, c) = validate_question(
            "  Pick one ",
            &[" A".into(), "A".into(), "B ".into()],
            Some("  "),
        )
        .unwrap();
        assert_eq!(
            (q.as_str(), o, c),
            ("Pick one", vec!["A".to_owned(), "B".to_owned()], None)
        );
        assert!(validate_question(" ", &[], None).is_err());
        assert!(validate_question("q", &[" ".into()], None).is_err());
        assert!(validate_question("q", &vec!["a".to_owned(); 13], None).is_err());
        assert!(validate_answer("  ").is_err());
        assert_eq!(validate_answer(" yes ").unwrap(), "yes");
    }

    #[test]
    fn the_registry_shape_is_camel_case() {
        let q = AgentQuestion {
            id: "q-1".into(),
            chat_id: "c".into(),
            question: "Ship?".into(),
            options: vec!["Yes".into()],
            context: None,
            blocking: false,
            status: QuestionStatus::Pending,
            answer: None,
            created_at: chrono::DateTime::from_timestamp_millis(1).unwrap(),
            resolved_at: None,
            resolved_by: None,
            delivered_at: None,
        };
        let v = serde_json::to_value(&q).unwrap();
        assert_eq!(v["chatId"], "c");
        assert_eq!(v["status"], "pending");
        let msg = answer_message(&q, " Yes ");
        assert_eq!(msg, "Answer to your question «Ship?» (question q-1): Yes");
    }
}
