//! Goal mode: the replicated state of a chat's `/goal`.
//!
//! A goal keeps one chat working turn after turn until an independent
//! verifier child chat judges the objective met (`docs/goal-mode.md`). The
//! state rides the chat's session doc (`meta.goal`, host-only writer) so every
//! client that syncs the doc can render it; mutations travel the command plane
//! as [`GoalCommand`]s and are executed by the chat's host.
//!
//! Everything here is additive and serde-defaulted: an older build that does
//! not know the `goal` key never reads it, and a newer build reading a goal
//! written by an older shape fills the gaps from `Default`.

use serde::{Deserialize, Serialize};

/// Longest objective the controller accepts, in Unicode scalar values.
pub const GOAL_OBJECTIVE_MAX_CHARS: usize = 4000;
/// Verdict history kept on the goal (oldest dropped first).
pub const GOAL_VERDICTS_MAX: usize = 20;
/// Rounds a goal may run before it pauses as budget-limited, unless the
/// caller picked another cap.
pub const GOAL_DEFAULT_MAX_ROUNDS: u32 = 25;
/// Ceiling for a caller-supplied round cap.
pub const GOAL_MAX_ROUNDS_CEILING: u32 = 200;
/// Longest derived title, in characters.
pub const GOAL_TITLE_MAX_CHARS: usize = 72;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum GoalStatus {
    /// The controller is driving the chat: a continuation is queued, a turn is
    /// running, or the next one is about to be sent.
    #[default]
    Active,
    /// Stopped without a verdict (user, interrupt, verifier failure, …).
    /// `Goal::reason` says why. Resumable.
    Paused,
    /// A verifier child chat is judging the objective.
    Verifying,
    /// The verifier passed the objective. Terminal.
    Complete,
    /// A round / token / time cap was reached. Resumable (extends the cap).
    BudgetLimited,
}

impl GoalStatus {
    /// The controller has more to do (or is waiting for a turn / verdict).
    pub fn is_running(self) -> bool {
        matches!(self, GoalStatus::Active | GoalStatus::Verifying)
    }

    /// Nothing further happens without a user action.
    pub fn is_stopped(self) -> bool {
        !self.is_running()
    }
}

/// Why a goal is not running. Machine-readable so UIs can pick copy and an
/// icon; `message` carries the human sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GoalReasonKind {
    /// The user paused it.
    User,
    /// The user stopped the running turn.
    Interrupted,
    /// The agent's turn ended in an error.
    TurnFailed,
    /// The verifier could not produce a verdict (timeout, crash, bad output).
    VerifierFailed,
    /// Two identical verdicts and no file or tool activity between them.
    NoProgress,
    /// The round cap was reached.
    MaxRounds,
    /// The token budget was reached.
    TokenBudget,
    /// The time budget was reached.
    TimeBudget,
    /// The chat is read-only: the goal is recorded but not pursued.
    ReadOnly,
    /// The engine restarted mid-turn and the turn did not resume.
    Restarted,
    /// The verifier passed.
    Verified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalReason {
    pub kind: GoalReasonKind,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VerdictOutcome {
    /// The verifier judged the objective met.
    Pass,
    /// The verifier judged it not met; `next_action` says what to do.
    NotSatisfied,
    /// No verdict: the verifier itself failed.
    Failed,
}

/// One verifier judgement (or failure) at a round boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalVerdict {
    /// The round that was judged (1-based).
    pub iteration: u32,
    pub outcome: VerdictOutcome,
    #[serde(default)]
    pub reason: String,
    /// The next smallest useful action — becomes the next round's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_action: Option<String>,
    /// Epoch millis.
    pub at: i64,
    /// The hidden child chat that judged it (opens from the panel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier_chat_id: Option<String>,
    /// Tokens the verifier spent (input + output), when the harness said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier_tokens: Option<u64>,
    /// Wall time the verification took.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GoalPendingKind {
    /// A continuation message was queued for this round; waiting for its turn.
    Turn,
    /// The round's turn finished; the verifier is judging it.
    Verify,
}

/// The controller's ledger: what the active goal is waiting for. Persisted so
/// a restarted host neither loses the round nor double-continues it — the
/// continuation's queue row / user message id is derived from the goal id and
/// round, so re-sending is idempotent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalPending {
    pub kind: GoalPendingKind,
    pub round: u32,
    /// Queue row id == transcript user message id of the round's prompt.
    pub message_id: String,
    /// Epoch millis the phase began (turn queued / verification started).
    pub started_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Goal {
    pub id: String,
    pub objective: String,
    /// Short label for the panel header and round 1; truncation of the
    /// objective's first line.
    #[serde(default)]
    pub summary_title: String,
    #[serde(default)]
    pub status: GoalStatus,
    /// Rounds started so far (1-based once the first prompt is queued).
    #[serde(default)]
    pub iteration: u32,
    /// Round cap before the goal pauses as budget-limited.
    #[serde(default = "default_max_rounds")]
    pub max_rounds: u32,
    /// Optional token cap (agent turns + verifier chats).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    /// Optional wall-clock cap, seconds of goal activity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_budget_seconds: Option<u64>,
    /// How many times the user resumed past a cap; each extends every cap by
    /// its original allowance (see [`Goal::effective_max_rounds`]).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub extensions: u32,
    /// Tokens the working agent spent on this goal's rounds.
    #[serde(default)]
    pub tokens_used: u64,
    /// Tokens verifier chats spent — accounted separately, counts toward the
    /// budget.
    #[serde(default)]
    pub verifier_tokens_used: u64,
    /// Seconds of goal activity (turns + verification), excluding pauses.
    #[serde(default)]
    pub time_used_seconds: u64,
    /// Epoch millis.
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<GoalReason>,
    /// Newest last, at most [`GOAL_VERDICTS_MAX`].
    #[serde(default)]
    pub verdicts: Vec<GoalVerdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<GoalPending>,
}

fn default_max_rounds() -> u32 {
    GOAL_DEFAULT_MAX_ROUNDS
}

fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

/// Caller-chosen limits for a new goal. `None` = the default.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rounds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_budget_seconds: Option<u64>,
}

/// A goal mutation, executed by the chat's host (command plane).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GoalCommand {
    /// Create a goal. With `replace` an existing running or paused goal is
    /// superseded; without it that is rejected so a stray `/goal text` cannot
    /// silently discard an active objective.
    Set {
        objective: String,
        #[serde(default)]
        limits: GoalLimits,
        #[serde(default)]
        replace: bool,
    },
    Pause,
    Resume,
    Clear,
}

/// Why a [`GoalCommand`] was refused. Display text is user-facing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalError {
    EmptyObjective,
    ObjectiveTooLong { chars: usize },
    InvalidLimit(&'static str),
    AlreadyExists,
    NoGoal,
    NotPausable(GoalStatus),
    NotResumable(GoalStatus),
}

impl std::fmt::Display for GoalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GoalError::EmptyObjective => f.write_str("the goal objective is empty"),
            GoalError::ObjectiveTooLong { chars } => write!(
                f,
                "the goal objective is {chars} characters; the limit is {GOAL_OBJECTIVE_MAX_CHARS}"
            ),
            GoalError::InvalidLimit(what) => write!(f, "invalid goal limit: {what}"),
            GoalError::AlreadyExists => f.write_str(
                "this chat already has a goal — replace it explicitly, or clear it first",
            ),
            GoalError::NoGoal => f.write_str("this chat has no goal"),
            GoalError::NotPausable(status) => {
                write!(f, "the goal is {status:?} and cannot be paused")
            }
            GoalError::NotResumable(status) => {
                write!(f, "the goal is {status:?} and cannot be resumed")
            }
        }
    }
}

impl std::error::Error for GoalError {}

impl Goal {
    /// Validate and build a fresh active goal. `id` and `now_ms` are injected
    /// so callers (and tests) control identity and time.
    pub fn new(
        id: impl Into<String>,
        objective: &str,
        limits: &GoalLimits,
        now_ms: i64,
    ) -> Result<Self, GoalError> {
        let objective = objective.trim();
        if objective.is_empty() {
            return Err(GoalError::EmptyObjective);
        }
        let chars = objective.chars().count();
        if chars > GOAL_OBJECTIVE_MAX_CHARS {
            return Err(GoalError::ObjectiveTooLong { chars });
        }
        let max_rounds = match limits.max_rounds {
            None => GOAL_DEFAULT_MAX_ROUNDS,
            Some(0) => return Err(GoalError::InvalidLimit("rounds must be at least 1")),
            Some(n) if n > GOAL_MAX_ROUNDS_CEILING => {
                return Err(GoalError::InvalidLimit("rounds above 200"));
            }
            Some(n) => n,
        };
        if limits.token_budget == Some(0) {
            return Err(GoalError::InvalidLimit("token budget must be positive"));
        }
        if limits.time_budget_seconds == Some(0) {
            return Err(GoalError::InvalidLimit("time budget must be positive"));
        }
        Ok(Self {
            id: id.into(),
            objective: objective.to_owned(),
            summary_title: title_from_objective(objective),
            status: GoalStatus::Active,
            iteration: 0,
            max_rounds,
            token_budget: limits.token_budget,
            time_budget_seconds: limits.time_budget_seconds,
            extensions: 0,
            tokens_used: 0,
            verifier_tokens_used: 0,
            time_used_seconds: 0,
            created_at: now_ms,
            updated_at: now_ms,
            reason: None,
            verdicts: Vec::new(),
            pending: None,
        })
    }

    /// Round cap after any "resume past the cap" extensions.
    pub fn effective_max_rounds(&self) -> u32 {
        self.max_rounds.saturating_mul(1 + self.extensions)
    }

    pub fn effective_token_budget(&self) -> Option<u64> {
        self.token_budget
            .map(|n| n.saturating_mul(u64::from(1 + self.extensions)))
    }

    pub fn effective_time_budget_seconds(&self) -> Option<u64> {
        self.time_budget_seconds
            .map(|n| n.saturating_mul(u64::from(1 + self.extensions)))
    }

    /// Agent + verifier tokens.
    pub fn total_tokens(&self) -> u64 {
        self.tokens_used.saturating_add(self.verifier_tokens_used)
    }

    /// The first limit already reached, if any. Rounds are judged by the
    /// caller (it depends on whether another round is about to start).
    pub fn exhausted_budget(&self) -> Option<GoalReasonKind> {
        if self
            .effective_token_budget()
            .is_some_and(|cap| self.total_tokens() >= cap)
        {
            return Some(GoalReasonKind::TokenBudget);
        }
        if self
            .effective_time_budget_seconds()
            .is_some_and(|cap| self.time_used_seconds >= cap)
        {
            return Some(GoalReasonKind::TimeBudget);
        }
        None
    }

    pub fn last_verdict(&self) -> Option<&GoalVerdict> {
        self.verdicts.last()
    }

    /// Append a verdict, keeping the newest [`GOAL_VERDICTS_MAX`].
    pub fn push_verdict(&mut self, verdict: GoalVerdict) {
        self.verdicts.push(verdict);
        let excess = self.verdicts.len().saturating_sub(GOAL_VERDICTS_MAX);
        if excess > 0 {
            self.verdicts.drain(..excess);
        }
    }

    /// The panel title of round `round`: round 1 is the goal title; later
    /// rounds are the previous verdict's next action.
    pub fn round_title(&self, round: u32) -> String {
        if round > 1
            && let Some(action) = self
                .verdicts
                .iter()
                .rev()
                .find(|v| v.iteration + 1 == round)
                .and_then(|v| v.next_action.as_deref())
                .map(str::trim)
                .filter(|a| !a.is_empty())
        {
            return action.to_owned();
        }
        if self.summary_title.is_empty() {
            title_from_objective(&self.objective)
        } else {
            self.summary_title.clone()
        }
    }

    /// Queue row / transcript message id of round `round`'s prompt.
    pub fn round_message_id(&self, round: u32) -> String {
        format!("goal-{}-r{round}", self.id)
    }

    /// Stamp a stop (`Paused` or `BudgetLimited`) with its reason.
    pub fn stop(&mut self, status: GoalStatus, kind: GoalReasonKind, message: impl Into<String>) {
        debug_assert!(status.is_stopped());
        self.status = status;
        self.pending = None;
        self.reason = Some(GoalReason {
            kind,
            message: message.into(),
        });
    }
}

/// Truncated first line of `objective`, on a character boundary.
pub fn title_from_objective(objective: &str) -> String {
    let line = objective
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    truncate_chars(line, GOAL_TITLE_MAX_CHARS)
}

/// `text` cut to `max` characters with an ellipsis when it was longer.
pub fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// Who authored a user-role transcript message when it was not the user.
/// Additive on the message entry (and on the queue row it came from): clients
/// render a compact marker for a known origin, an unknown future kind falls
/// back to the plain bubble.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MessageOrigin {
    /// A goal-mode round prompt.
    Goal {
        goal_id: String,
        round: u32,
        /// The round's title (the verdict's next action; goal title on round 1).
        #[serde(default)]
        title: String,
    },
    /// A goal lifecycle marker (system-role transcript entry): set, a round's
    /// verdict, completion, a pause… The entry's text part carries the same
    /// information in words, which is what a client that predates this kind
    /// shows.
    GoalEvent {
        goal_id: String,
        event: GoalEventKind,
        /// The round the event concerns (0 when not about a round).
        #[serde(default)]
        round: u32,
        /// Goal title, or the next action of a not-satisfied verdict.
        #[serde(default)]
        title: String,
        /// The verifier's reason, or the pause / limit message.
        #[serde(default)]
        detail: String,
        /// The verifier child chat, when the event is a verdict.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verifier_chat_id: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GoalEventKind {
    Set,
    Resumed,
    Paused,
    Cleared,
    NotSatisfied,
    Complete,
    BudgetLimited,
    VerifierFailed,
}

impl MessageOrigin {
    /// The text a marker entry carries as its only text part.
    pub fn goal_event_text(event: GoalEventKind, round: u32, title: &str, detail: &str) -> String {
        let head = match event {
            GoalEventKind::Set => format!("Goal set: {title}"),
            GoalEventKind::Resumed => "Goal resumed".to_owned(),
            GoalEventKind::Paused => "Goal paused".to_owned(),
            GoalEventKind::Cleared => "Goal cleared".to_owned(),
            GoalEventKind::NotSatisfied => format!("Round {round}: not satisfied"),
            GoalEventKind::Complete => format!("Goal complete after {round} round(s)"),
            GoalEventKind::BudgetLimited => "Goal stopped at its limit".to_owned(),
            GoalEventKind::VerifierFailed => format!("Round {round}: verifier failed"),
        };
        if detail.trim().is_empty() {
            head
        } else {
            format!("{head} — {}", detail.trim())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal() -> Goal {
        Goal::new("g1", "Ship the thing", &GoalLimits::default(), 10).unwrap()
    }

    #[test]
    fn rejects_empty_and_oversized_objectives() {
        assert_eq!(
            Goal::new("g", "  \n ", &GoalLimits::default(), 0).unwrap_err(),
            GoalError::EmptyObjective
        );
        let long = "x".repeat(GOAL_OBJECTIVE_MAX_CHARS + 1);
        assert_eq!(
            Goal::new("g", &long, &GoalLimits::default(), 0).unwrap_err(),
            GoalError::ObjectiveTooLong {
                chars: GOAL_OBJECTIVE_MAX_CHARS + 1
            }
        );
        // The limit counts characters, not bytes.
        let wide = "é".repeat(GOAL_OBJECTIVE_MAX_CHARS);
        assert!(Goal::new("g", &wide, &GoalLimits::default(), 0).is_ok());
    }

    #[test]
    fn validates_limits() {
        let bad = |limits: GoalLimits| Goal::new("g", "x", &limits, 0).unwrap_err();
        assert!(matches!(
            bad(GoalLimits {
                max_rounds: Some(0),
                ..Default::default()
            }),
            GoalError::InvalidLimit(_)
        ));
        assert!(matches!(
            bad(GoalLimits {
                token_budget: Some(0),
                ..Default::default()
            }),
            GoalError::InvalidLimit(_)
        ));
        assert!(matches!(
            bad(GoalLimits {
                max_rounds: Some(GOAL_MAX_ROUNDS_CEILING + 1),
                ..Default::default()
            }),
            GoalError::InvalidLimit(_)
        ));
    }

    #[test]
    fn title_is_the_truncated_first_line() {
        assert_eq!(
            title_from_objective("\n\n  Fix the build  \nmore"),
            "Fix the build"
        );
        let long = "word ".repeat(40);
        let title = title_from_objective(&long);
        assert!(title.chars().count() <= GOAL_TITLE_MAX_CHARS);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn extensions_scale_every_cap() {
        let mut g = Goal::new(
            "g",
            "x",
            &GoalLimits {
                max_rounds: Some(5),
                token_budget: Some(1000),
                time_budget_seconds: Some(60),
            },
            0,
        )
        .unwrap();
        assert_eq!(g.effective_max_rounds(), 5);
        g.extensions = 1;
        assert_eq!(g.effective_max_rounds(), 10);
        assert_eq!(g.effective_token_budget(), Some(2000));
        assert_eq!(g.effective_time_budget_seconds(), Some(120));
    }

    #[test]
    fn budget_counts_verifier_tokens_and_time() {
        let mut g = goal();
        g.token_budget = Some(100);
        g.tokens_used = 60;
        assert_eq!(g.exhausted_budget(), None);
        g.verifier_tokens_used = 40;
        assert_eq!(g.exhausted_budget(), Some(GoalReasonKind::TokenBudget));
        g.token_budget = None;
        g.time_budget_seconds = Some(5);
        g.time_used_seconds = 5;
        assert_eq!(g.exhausted_budget(), Some(GoalReasonKind::TimeBudget));
    }

    #[test]
    fn verdict_history_is_bounded_and_titles_follow_it() {
        let mut g = goal();
        for i in 1..=(GOAL_VERDICTS_MAX as u32 + 5) {
            g.push_verdict(GoalVerdict {
                iteration: i,
                outcome: VerdictOutcome::NotSatisfied,
                reason: "no".into(),
                next_action: Some(format!("step {i}")),
                at: i64::from(i),
                verifier_chat_id: None,
                verifier_tokens: None,
                verifier_ms: None,
            });
        }
        assert_eq!(g.verdicts.len(), GOAL_VERDICTS_MAX);
        assert_eq!(g.verdicts[0].iteration, 6);
        assert_eq!(g.round_title(1), "Ship the thing");
        assert_eq!(g.round_title(8), "step 7");
        // Rounds whose verdict aged out fall back to the goal title.
        assert_eq!(g.round_title(2), "Ship the thing");
    }

    #[test]
    fn round_message_ids_are_stable_per_goal_and_round() {
        let g = goal();
        assert_eq!(g.round_message_id(3), "goal-g1-r3");
    }

    #[test]
    fn old_and_partial_shapes_decode() {
        // Only the required identity: everything else defaults.
        let g: Goal = serde_json::from_str(r#"{"id":"g","objective":"o"}"#).unwrap();
        assert_eq!(g.status, GoalStatus::Active);
        assert_eq!(g.max_rounds, GOAL_DEFAULT_MAX_ROUNDS);
        assert!(g.verdicts.is_empty());
        // Unknown fields from a newer writer are ignored.
        let g: Goal =
            serde_json::from_str(r#"{"id":"g","objective":"o","future":{"a":1}}"#).unwrap();
        assert_eq!(g.objective, "o");
    }

    #[test]
    fn goal_round_trips_through_json() {
        let mut g = goal();
        g.stop(
            GoalStatus::Paused,
            GoalReasonKind::VerifierFailed,
            "verifier timed out",
        );
        g.push_verdict(GoalVerdict {
            iteration: 1,
            outcome: VerdictOutcome::Failed,
            reason: "timeout".into(),
            next_action: None,
            at: 5,
            verifier_chat_id: Some("v1".into()),
            verifier_tokens: Some(12),
            verifier_ms: Some(900),
        });
        let json = serde_json::to_string(&g).unwrap();
        assert_eq!(serde_json::from_str::<Goal>(&json).unwrap(), g);
        assert!(json.contains("\"status\":\"paused\""));
        assert!(json.contains("\"kind\":\"verifierFailed\""));
    }

    #[test]
    fn commands_use_a_flat_action_tag() {
        let set = GoalCommand::Set {
            objective: "x".into(),
            limits: GoalLimits {
                max_rounds: Some(3),
                ..Default::default()
            },
            replace: true,
        };
        let value = serde_json::to_value(&set).unwrap();
        assert_eq!(value["action"], "set");
        assert_eq!(value["limits"]["maxRounds"], 3);
        assert_eq!(serde_json::from_value::<GoalCommand>(value).unwrap(), set);
        assert_eq!(
            serde_json::from_str::<GoalCommand>(r#"{"action":"pause"}"#).unwrap(),
            GoalCommand::Pause
        );
    }

    #[test]
    fn unknown_origin_kinds_decode_as_unknown() {
        let known: MessageOrigin =
            serde_json::from_str(r#"{"kind":"goal","goalId":"g","round":2,"title":"t"}"#).unwrap();
        assert_eq!(
            known,
            MessageOrigin::Goal {
                goal_id: "g".into(),
                round: 2,
                title: "t".into()
            }
        );
        let future: MessageOrigin =
            serde_json::from_str(r#"{"kind":"workflow","runId":"r"}"#).unwrap();
        assert_eq!(future, MessageOrigin::Unknown);
    }
}
