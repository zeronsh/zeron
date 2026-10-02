//! Goal mode's presentation, as plain data: the `/goal` line, the status chip,
//! the header text, and the transcript markers goal and workflow machinery
//! leave behind. No UI types, so the desktop tray and the mobile strip read
//! the same words (`docs/goal-mode.md`).

use crate::view::one_line;
use crate::{
    GOAL_OBJECTIVE_MAX_CHARS, Goal, GoalCommand, GoalEventKind, GoalLimits, GoalStatus,
    MessageOrigin, WorkflowEventMarker, WorkflowStatus,
};

// ---------------------------------------------------------------------------
// `/goal` input
// ---------------------------------------------------------------------------

/// What a typed `/goal …` line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalInput {
    /// Bare `/goal`: show and focus the tray.
    Show,
    Set {
        objective: String,
        replace: bool,
    },
    Pause,
    Resume,
    Clear,
}

/// Parse a composer line. `None` = not a goal command (an ordinary message);
/// `Some(Err)` = a goal command that cannot be sent, with the reason to show.
///
/// Only a line that *starts* with `/goal` counts — leading indentation is how
/// the composer lets people send a slash-prefixed line literally. The control
/// words (`pause`, `resume`, `clear`) must be the whole argument, so
/// `/goal pause the deploy script` is an objective, not a pause.
pub fn parse_goal_input(text: &str) -> Option<Result<GoalInput, String>> {
    let rest = text.strip_prefix("/goal")?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None; // `/goals`, `/goalpost`
    }
    let arg = rest.trim();
    let lower = arg.to_lowercase();
    Some(match lower.as_str() {
        "" => Ok(GoalInput::Show),
        "pause" => Ok(GoalInput::Pause),
        "resume" => Ok(GoalInput::Resume),
        "clear" => Ok(GoalInput::Clear),
        "replace" => Err("Add the new objective after `replace`.".into()),
        _ => {
            let (objective, replace) = match lower
                .strip_prefix("replace")
                .filter(|r| r.starts_with(char::is_whitespace))
            {
                Some(_) => (arg["replace".len()..].trim(), true),
                None => (arg, false),
            };
            let chars = objective.chars().count();
            if chars > GOAL_OBJECTIVE_MAX_CHARS {
                Err(format!(
                    "The objective is {chars} characters; the limit is {GOAL_OBJECTIVE_MAX_CHARS}."
                ))
            } else {
                Ok(GoalInput::Set {
                    objective: objective.to_owned(),
                    replace,
                })
            }
        }
    })
}

/// Shown when `/goal <objective>` would silently replace a live goal.
pub const GOAL_REPLACE_HINT: &str =
    "This chat already has a goal. Use /goal replace <objective>, or /goal clear first.";
/// Shown for `/goal` (and pause / resume / clear) in a chat without a goal.
pub const NO_GOAL_HINT: &str = "This chat has no goal. Type /goal followed by an objective.";

/// A plain `/goal <text>` must not discard an objective that is still
/// running or paused; the user asks for that with `replace`.
pub fn set_needs_replace(goal: Option<&Goal>) -> bool {
    goal.is_some_and(|g| !matches!(g.status, GoalStatus::Complete | GoalStatus::BudgetLimited))
}

impl GoalInput {
    pub fn command(&self) -> Option<GoalCommand> {
        Some(match self {
            GoalInput::Show => return None,
            GoalInput::Set { objective, replace } => GoalCommand::Set {
                objective: objective.clone(),
                limits: GoalLimits::default(),
                replace: *replace,
            },
            GoalInput::Pause => GoalCommand::Pause,
            GoalInput::Resume => GoalCommand::Resume,
            GoalInput::Clear => GoalCommand::Clear,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Accent,
    Success,
    Warning,
    Muted,
}

/// The status chip: label and tone.
pub fn chip(goal: &Goal) -> (&'static str, Tone) {
    match goal.status {
        GoalStatus::Active => ("Active", Tone::Accent),
        GoalStatus::Verifying => ("Verifying", Tone::Accent),
        GoalStatus::Paused => ("Paused", Tone::Muted),
        GoalStatus::Complete => ("Complete", Tone::Success),
        GoalStatus::BudgetLimited => ("Budget reached", Tone::Warning),
    }
}

/// Seconds of goal activity, including the phase in progress.
pub fn elapsed_seconds(goal: &Goal, now_ms: i64) -> u64 {
    let running = match (&goal.pending, goal.status.is_running()) {
        (Some(p), true) => (now_ms.saturating_sub(p.started_at).max(0) / 1000) as u64,
        _ => 0,
    };
    goal.time_used_seconds.saturating_add(running)
}

pub use crate::view::format_tokens;

/// `Round 3 of 25 · 12.4k tokens (verifier 1.1k) · 4m 12s`, with the caps that
/// are set.
pub fn meta_line(goal: &Goal, now_ms: i64) -> String {
    let mut parts = vec![format!(
        "Round {} of {}",
        goal.iteration.max(1),
        goal.effective_max_rounds()
    )];
    let tokens = match goal.effective_token_budget() {
        Some(cap) => format!(
            "{} of {} tokens",
            format_tokens(goal.total_tokens()),
            format_tokens(cap)
        ),
        None => format!("{} tokens", format_tokens(goal.total_tokens())),
    };
    parts.push(if goal.verifier_tokens_used > 0 {
        format!(
            "{tokens} (verifier {})",
            format_tokens(goal.verifier_tokens_used)
        )
    } else {
        tokens
    });
    let elapsed = crate::view::format_elapsed(elapsed_seconds(goal, now_ms) as i64);
    parts.push(match goal.effective_time_budget_seconds() {
        Some(cap) => format!("{elapsed} of {}", crate::view::format_elapsed(cap as i64)),
        None => elapsed,
    });
    parts.join(" · ")
}

/// Where the collapsed header's single line of text comes from.
pub fn headline(goal: &Goal) -> (String, Tone) {
    match (&goal.status, &goal.reason) {
        (GoalStatus::Complete, _) => ("Verified".into(), Tone::Success),
        (GoalStatus::Paused, Some(r)) => (r.message.clone(), Tone::Muted),
        (GoalStatus::BudgetLimited, Some(r)) => (r.message.clone(), Tone::Warning),
        _ => (
            format!(
                "Round {}: {}",
                goal.iteration.max(1),
                goal.round_title(goal.iteration.max(1))
            ),
            Tone::Muted,
        ),
    }
}

/// A transcript marker: either a round's prompt or a lifecycle event.
#[derive(Debug, Clone, PartialEq)]
pub struct GoalMarker {
    pub kind: MarkerKind,
    pub round: u32,
    pub title: String,
    pub detail: String,
    pub verifier_chat_id: Option<String>,
    /// The workflow run a workflow marker belongs to (the transcript folds
    /// its end markers into the run's card).
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    /// A round's prompt, sent by the controller.
    Round,
    Event(GoalEventKind),
    /// A workflow lifecycle marker (started, completed, stopped…).
    Workflow(WorkflowEventMarker),
    /// A workflow's machine message to the parent agent (its result, or an
    /// agent's question), shown compactly: the full text is for the model.
    WorkflowMessage(WorkflowStatus),
}

impl GoalMarker {
    /// The marker an entry stands for, when it is goal machinery rather than
    /// a message: a controller-sent round prompt (a user entry) or a
    /// lifecycle event (a system entry). `text` is the entry's first text
    /// part (a workflow question reads its question from it).
    pub fn from_origin(origin: &MessageOrigin, is_user: bool, text: &str) -> Option<GoalMarker> {
        match origin {
            MessageOrigin::Goal { round, title, .. } if is_user => Some(GoalMarker {
                kind: MarkerKind::Round,
                round: *round,
                title: title.clone(),
                detail: String::new(),
                verifier_chat_id: None,
                run_id: None,
            }),
            MessageOrigin::GoalEvent {
                event,
                round,
                title,
                detail,
                verifier_chat_id,
                ..
            } => Some(GoalMarker {
                kind: MarkerKind::Event(*event),
                round: *round,
                title: title.clone(),
                detail: detail.clone(),
                verifier_chat_id: verifier_chat_id.clone(),
                run_id: None,
            }),
            MessageOrigin::WorkflowEvent {
                marker,
                name,
                detail,
                run_id,
            } => Some(GoalMarker {
                kind: MarkerKind::Workflow(*marker),
                round: 0,
                title: name.clone(),
                detail: detail.clone(),
                verifier_chat_id: None,
                run_id: Some(run_id.clone()),
            }),
            MessageOrigin::Workflow {
                name,
                status,
                run_id,
            } if is_user => {
                // A question row shows the question; a result row is just a
                // label (the end marker above already carries the summary).
                let detail = if text.starts_with("[Workflow question]") {
                    text.lines().nth(2).unwrap_or_default().to_owned()
                } else {
                    String::new()
                };
                Some(GoalMarker {
                    kind: MarkerKind::WorkflowMessage(*status),
                    round: 0,
                    title: name.clone(),
                    detail,
                    verifier_chat_id: None,
                    run_id: Some(run_id.clone()),
                })
            }
            _ => None,
        }
    }

    /// The one-line label: `Goal · round 3: Add the missing test`.
    pub fn label(&self) -> String {
        use GoalEventKind::*;
        match self.kind {
            MarkerKind::Round => format!("Goal · round {}: {}", self.round, one_line(&self.title)),
            MarkerKind::Event(Set) => format!("Goal set · {}", one_line(&self.title)),
            MarkerKind::Event(Resumed) => "Goal resumed".into(),
            MarkerKind::Event(Paused) => "Goal paused".into(),
            MarkerKind::Event(Cleared) => "Goal cleared".into(),
            MarkerKind::Event(NotSatisfied) => {
                format!("Verifier · round {} not satisfied", self.round)
            }
            MarkerKind::Event(Complete) => format!("Goal complete · {} rounds", self.round),
            MarkerKind::Event(BudgetLimited) => "Goal stopped at its limit".into(),
            MarkerKind::Event(VerifierFailed) => {
                format!("Verifier · round {} failed", self.round)
            }
            MarkerKind::Workflow(marker) => crate::workflow_marker_text(marker, &self.title, ""),
            MarkerKind::WorkflowMessage(WorkflowStatus::Running) => {
                format!("Workflow agent asks a question · {}", one_line(&self.title))
            }
            MarkerKind::WorkflowMessage(status) => {
                let word = match status {
                    WorkflowStatus::Completed => "completed",
                    WorkflowStatus::Errored => "failed",
                    _ => "stopped",
                };
                format!(
                    "Result of workflow {} ({word}) sent to the agent",
                    one_line(&self.title)
                )
            }
        }
    }

    pub fn tone(&self) -> Tone {
        use GoalEventKind::*;
        match self.kind {
            MarkerKind::Event(Complete) => Tone::Success,
            MarkerKind::Workflow(WorkflowEventMarker::Completed)
            | MarkerKind::WorkflowMessage(WorkflowStatus::Completed) => Tone::Success,
            MarkerKind::Workflow(
                WorkflowEventMarker::Errored
                | WorkflowEventMarker::Stopped
                | WorkflowEventMarker::Denied,
            )
            | MarkerKind::WorkflowMessage(WorkflowStatus::Errored | WorkflowStatus::Stopped) => {
                Tone::Warning
            }
            MarkerKind::Workflow(_) | MarkerKind::WorkflowMessage(_) => Tone::Accent,
            MarkerKind::Event(BudgetLimited | VerifierFailed) => Tone::Warning,
            MarkerKind::Round | MarkerKind::Event(Set | Resumed) => Tone::Accent,
            _ => Tone::Muted,
        }
    }

    /// The second line (verdict reason, pause message, next action).
    pub fn detail_line(&self) -> Option<String> {
        let detail = one_line(&self.detail);
        let text = match self.kind {
            MarkerKind::Event(GoalEventKind::NotSatisfied) if !self.title.is_empty() => {
                if detail.is_empty() {
                    format!("Next: {}", one_line(&self.title))
                } else {
                    format!("{detail} — next: {}", one_line(&self.title))
                }
            }
            _ => detail,
        };
        (!text.is_empty()).then_some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GoalReasonKind, GoalVerdict, VerdictOutcome};

    fn goal() -> Goal {
        Goal::new("g1", "Ship the thing", &GoalLimits::default(), 0).unwrap()
    }

    #[test]
    fn goal_lines_parse() {
        assert_eq!(parse_goal_input("/goal"), Some(Ok(GoalInput::Show)));
        assert_eq!(parse_goal_input("/goal  "), Some(Ok(GoalInput::Show)));
        assert_eq!(parse_goal_input("/goal pause"), Some(Ok(GoalInput::Pause)));
        assert_eq!(
            parse_goal_input("/goal Resume"),
            Some(Ok(GoalInput::Resume))
        );
        assert_eq!(parse_goal_input("/goal clear"), Some(Ok(GoalInput::Clear)));
        assert_eq!(
            parse_goal_input("/goal fix the build"),
            Some(Ok(GoalInput::Set {
                objective: "fix the build".into(),
                replace: false
            }))
        );
        assert_eq!(
            parse_goal_input("/goal replace ship it\nsecond line"),
            Some(Ok(GoalInput::Set {
                objective: "ship it\nsecond line".into(),
                replace: true
            }))
        );
    }

    #[test]
    fn control_words_must_be_the_whole_argument() {
        // An objective that merely starts with a control word is an objective.
        assert_eq!(
            parse_goal_input("/goal pause the deploy script"),
            Some(Ok(GoalInput::Set {
                objective: "pause the deploy script".into(),
                replace: false
            }))
        );
        assert_eq!(
            parse_goal_input("/goal replacement strategy"),
            Some(Ok(GoalInput::Set {
                objective: "replacement strategy".into(),
                replace: false
            }))
        );
        assert!(matches!(parse_goal_input("/goal replace"), Some(Err(_))));
    }

    #[test]
    fn only_a_leading_goal_command_counts() {
        assert_eq!(parse_goal_input("hello /goal x"), None);
        assert_eq!(
            parse_goal_input(" /goal x"),
            None,
            "indentation sends literally"
        );
        assert_eq!(parse_goal_input("/goals"), None);
        assert_eq!(parse_goal_input("/goalpost"), None);
        assert_eq!(parse_goal_input("/compact"), None);
    }

    #[test]
    fn oversized_objectives_are_refused_before_sending() {
        let long = format!("/goal {}", "x".repeat(GOAL_OBJECTIVE_MAX_CHARS + 1));
        assert!(matches!(parse_goal_input(&long), Some(Err(m)) if m.contains("limit")));
        let ok = format!("/goal {}", "é".repeat(GOAL_OBJECTIVE_MAX_CHARS));
        assert!(matches!(parse_goal_input(&ok), Some(Ok(_))));
    }

    #[test]
    fn status_chips_and_headlines() {
        let mut g = goal();
        assert_eq!(chip(&g), ("Active", Tone::Accent));
        g.iteration = 2;
        g.push_verdict(GoalVerdict {
            iteration: 1,
            outcome: VerdictOutcome::NotSatisfied,
            reason: "no".into(),
            next_action: Some("Add the test".into()),
            at: 1,
            verifier_chat_id: None,
            verifier_tokens: None,
            verifier_ms: None,
        });
        assert_eq!(headline(&g).0, "Round 2: Add the test");
        g.stop(
            GoalStatus::BudgetLimited,
            GoalReasonKind::MaxRounds,
            "Reached the round limit",
        );
        assert_eq!(chip(&g), ("Budget reached", Tone::Warning));
        assert_eq!(
            headline(&g),
            ("Reached the round limit".into(), Tone::Warning)
        );
        g.status = GoalStatus::Complete;
        assert_eq!(chip(&g).0, "Complete");
        assert_eq!(headline(&g).0, "Verified");
    }

    #[test]
    fn elapsed_adds_the_running_phase_only_while_running() {
        let mut g = goal();
        g.time_used_seconds = 60;
        g.pending = Some(crate::GoalPending {
            kind: crate::GoalPendingKind::Turn,
            round: 1,
            message_id: "m".into(),
            started_at: 10_000,
        });
        assert_eq!(elapsed_seconds(&g, 25_000), 75);
        g.status = GoalStatus::Paused;
        assert_eq!(elapsed_seconds(&g, 25_000), 60);
    }

    #[test]
    fn token_and_meta_formatting() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(12_400), "12.4k");
        assert_eq!(format_tokens(50_000), "50k");
        assert_eq!(format_tokens(2_100_000), "2.1M");
        let mut g = goal();
        g.iteration = 3;
        g.tokens_used = 12_000;
        g.verifier_tokens_used = 1_000;
        g.token_budget = Some(50_000);
        g.time_used_seconds = 252;
        assert_eq!(
            meta_line(&g, 0),
            "Round 3 of 25 · 13k of 50k tokens (verifier 1k) · 4m 12s"
        );
    }
}
