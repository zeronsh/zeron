//! What an agent scheduled for itself inside its own process: Claude's
//! `ScheduleWakeup` (a `/loop`'s next turn) and its session crons
//! (`CronCreate`). Both die with the process, so an idle agent holding one
//! is never stopped (see the idle reaper in `sessions`).
//!
//! Read from the agent's own tool calls and their results (Claude Code
//! 2.1.322's tool schemas):
//!
//! - `ScheduleWakeup { delaySeconds, prompt, reason }` sets the loop's one
//!   next wake-up, `delaySeconds` from now, clamped to [60, 3600] by the
//!   CLI; each call replaces the last, and `{ stop: true }` ends the loop.
//!   The CLI never says which wake turn a wake-up started (a background
//!   task ending starts one too), so the wake-up counts as pending until
//!   its time has come: a wake turn from then on is its own, and past
//!   [`WAKE_GRACE`] it has fired whether or not a turn was seen.
//! - `CronCreate` answers with the job's id; `CronDelete { id }` cancels
//!   that job. A cron counts from a successful create until a successful
//!   delete of its id, each id once.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use tokio::time::Instant;
use zeron_proto::{AgentEvent, ToolCall};

/// How long past its time a wake-up is still waited for.
pub const WAKE_GRACE: Duration = Duration::from_secs(60);
/// A wake turn this close before a wake-up's time is taken as its firing.
const WAKE_EARLY: Duration = Duration::from_secs(10);
/// `delaySeconds` as the CLI clamps it.
const WAKE_DELAY: (f64, f64) = (60.0, 3600.0);

/// A pending wake-up: when it fires, if its call said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Wakeup {
    due: Option<Instant>,
}

#[derive(Default, Debug)]
pub struct SelfScheduled {
    wakeup: Option<Wakeup>,
    /// The last `ScheduleWakeup` call and the wake-up pending before it,
    /// pending again if the call fails.
    wakeup_call: Option<(String, Option<Wakeup>)>,
    /// `CronCreate` calls awaiting their result.
    cron_calls: HashSet<String>,
    /// `CronDelete` calls awaiting their result, with the id each cancels.
    delete_calls: HashMap<String, String>,
    /// Each live cron's create result (it names the job's id).
    crons: Vec<String>,
    /// Ids already cancelled: a second delete of one changes nothing.
    deleted: HashSet<String>,
}

impl SelfScheduled {
    /// Take in one of the agent's events (its own, not a subagent's).
    pub fn observe(&mut self, event: &AgentEvent, now: Instant) {
        match event {
            AgentEvent::ToolCall {
                id,
                call: ToolCall::Unknown { name, input },
            } => match name.as_str() {
                "ScheduleWakeup" => {
                    let field = |key: &str| input.as_ref().and_then(|input| input.get(key));
                    let next = if field("stop").and_then(|v| v.as_bool()) == Some(true) {
                        None
                    } else {
                        let delay = field("delaySeconds")
                            .and_then(|v| v.as_f64())
                            .filter(|delay| delay.is_finite());
                        Some(Wakeup {
                            due: delay.map(|delay| {
                                now + Duration::from_secs_f64(
                                    delay.clamp(WAKE_DELAY.0, WAKE_DELAY.1),
                                )
                            }),
                        })
                    };
                    // (A harness may send a call again as it learns more:
                    // what was pending before it is still what it replaced.)
                    let before = match &self.wakeup_call {
                        Some((call, before)) if call == id => *before,
                        _ => self.wakeup,
                    };
                    self.wakeup_call = Some((id.clone(), before));
                    self.wakeup = next;
                }
                "CronCreate" => {
                    self.cron_calls.insert(id.clone());
                }
                "CronDelete" => {
                    if let Some(cron) = input
                        .as_ref()
                        .and_then(|input| input.get("id"))
                        .and_then(|v| v.as_str())
                    {
                        self.delete_calls.insert(id.clone(), cron.to_owned());
                    }
                }
                _ => {}
            },
            AgentEvent::ToolResult {
                id,
                is_error,
                output,
                ..
            } => {
                if let Some((call, before)) = &self.wakeup_call
                    && call == id
                {
                    if *is_error {
                        self.wakeup = *before;
                    }
                    self.wakeup_call = None;
                }
                if self.cron_calls.remove(id) && !is_error {
                    self.crons.push(output.clone().unwrap_or_default());
                }
                if let Some(cron) = self.delete_calls.remove(id)
                    && !is_error
                    && self.deleted.insert(cron.clone())
                {
                    // The cron its create result names; failing that (a
                    // result that doesn't say), the oldest, as a count would.
                    let at = self
                        .crons
                        .iter()
                        .position(|text| names(text, &cron))
                        .or_else(|| (!self.crons.is_empty()).then_some(0));
                    if let Some(at) = at {
                        self.crons.remove(at);
                    }
                }
            }
            _ => {}
        }
    }

    /// The runtime started a turn on its own (a wake turn): the wake-up's,
    /// if its time has come, or one whose time no call said.
    pub fn woke(&mut self, now: Instant) {
        if self
            .wakeup
            .is_some_and(|w| w.due.is_none_or(|due| now + WAKE_EARLY >= due))
        {
            self.wakeup = None;
        }
    }

    /// Why the agent must not be stopped now, if anything it scheduled is
    /// still to come.
    pub fn pending(&self, now: Instant) -> Option<&'static str> {
        if self
            .wakeup
            .is_some_and(|w| w.due.is_none_or(|due| now < due + WAKE_GRACE))
        {
            Some("the agent scheduled a wake-up")
        } else if !self.crons.is_empty() {
            Some("the agent scheduled a cron")
        } else {
            None
        }
    }
}

/// Whether `text` names `id` as a whole word.
fn names(text: &str, id: &str) -> bool {
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-');
    !id.is_empty()
        && text.match_indices(id).any(|(at, _)| {
            !word(text[..at].chars().next_back()) && !word(text[at + id.len()..].chars().next())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, name: &str, input: serde_json::Value) -> AgentEvent {
        AgentEvent::ToolCall {
            id: id.into(),
            call: ToolCall::Unknown {
                name: name.into(),
                input: Some(input),
            },
        }
    }

    fn result(id: &str, is_error: bool, output: &str) -> AgentEvent {
        AgentEvent::ToolResult {
            id: id.into(),
            is_error,
            output: Some(output.into()),
            diff: None,
            preview_paths: Vec::new(),
            image_payloads: Vec::new(),
            exit_code: None,
        }
    }

    fn mins(n: u64) -> Duration {
        Duration::from_secs(n * 60)
    }

    #[test]
    fn a_wake_up_is_pending_until_its_time_whatever_wakes_the_agent_before() {
        let t0 = Instant::now();
        let mut s = SelfScheduled::default();
        s.observe(
            &call(
                "w1",
                "ScheduleWakeup",
                serde_json::json!({ "delaySeconds": 1200 }),
            ),
            t0,
        );
        s.observe(&result("w1", false, "scheduled"), t0);
        // A background task's wake turn, long before: still pending.
        s.woke(t0 + mins(2));
        assert!(s.pending(t0 + mins(3)).is_some());
        // Its own wake turn.
        s.woke(t0 + mins(20));
        assert_eq!(s.pending(t0 + mins(20)), None);
    }

    #[test]
    fn a_wake_up_no_turn_was_seen_for_lapses_after_its_grace() {
        let t0 = Instant::now();
        let mut s = SelfScheduled::default();
        // Clamped up to 60 s by the CLI.
        s.observe(
            &call(
                "w1",
                "ScheduleWakeup",
                serde_json::json!({ "delaySeconds": 5 }),
            ),
            t0,
        );
        assert!(s.pending(t0 + Duration::from_secs(100)).is_some());
        assert_eq!(s.pending(t0 + Duration::from_secs(60) + WAKE_GRACE), None);
    }

    #[test]
    fn each_call_replaces_the_wake_up_and_stop_ends_the_loop() {
        let t0 = Instant::now();
        let mut s = SelfScheduled::default();
        for (i, n) in [600, 900, 1200].into_iter().enumerate() {
            s.observe(
                &call(
                    &format!("w{i}"),
                    "ScheduleWakeup",
                    serde_json::json!({ "delaySeconds": n }),
                ),
                t0,
            );
        }
        s.woke(t0 + mins(20));
        assert_eq!(s.pending(t0 + mins(20)), None, "one wake-up, not three");
        s.observe(
            &call(
                "w3",
                "ScheduleWakeup",
                serde_json::json!({ "delaySeconds": 600 }),
            ),
            t0,
        );
        s.observe(
            &call("w4", "ScheduleWakeup", serde_json::json!({ "stop": true })),
            t0,
        );
        assert_eq!(s.pending(t0), None);
    }

    #[test]
    fn a_failed_wake_up_call_leaves_the_one_before() {
        let t0 = Instant::now();
        let mut s = SelfScheduled::default();
        s.observe(
            &call(
                "w1",
                "ScheduleWakeup",
                serde_json::json!({ "delaySeconds": 600 }),
            ),
            t0,
        );
        s.observe(
            &call("w2", "ScheduleWakeup", serde_json::json!({ "stop": true })),
            t0,
        );
        s.observe(&result("w2", true, "error"), t0);
        assert!(s.pending(t0).is_some());
        // Without a delay (no input seen): pending until a wake turn.
        let mut s = SelfScheduled::default();
        s.observe(
            &AgentEvent::ToolCall {
                id: "w".into(),
                call: ToolCall::Unknown {
                    name: "ScheduleWakeup".into(),
                    input: None,
                },
            },
            t0,
        );
        assert!(s.pending(t0 + mins(600)).is_some());
        s.woke(t0 + mins(1));
        assert_eq!(s.pending(t0 + mins(1)), None);
    }

    #[test]
    fn crons_count_by_id_and_a_second_delete_of_one_changes_nothing() {
        let t0 = Instant::now();
        let mut s = SelfScheduled::default();
        s.observe(
            &call(
                "c1",
                "CronCreate",
                serde_json::json!({ "cron": "*/5 * * * *" }),
            ),
            t0,
        );
        s.observe(
            &result(
                "c1",
                false,
                "Scheduled recurring job a1b2c3d4 (*/5 * * * *)",
            ),
            t0,
        );
        s.observe(
            &call(
                "c2",
                "CronCreate",
                serde_json::json!({ "cron": "7 * * * *" }),
            ),
            t0,
        );
        s.observe(
            &result("c2", false, "Scheduled recurring job e5f6a7b8 (7 * * * *)"),
            t0,
        );
        // A create that failed holds nothing.
        s.observe(&call("c3", "CronCreate", serde_json::json!({})), t0);
        s.observe(&result("c3", true, "invalid cron"), t0);
        for call_id in ["d1", "d2"] {
            s.observe(
                &call(
                    call_id,
                    "CronDelete",
                    serde_json::json!({ "id": "a1b2c3d4" }),
                ),
                t0,
            );
            s.observe(&result(call_id, false, "Cancelled a1b2c3d4"), t0);
        }
        assert_eq!(s.pending(t0), Some("the agent scheduled a cron"));
        // A failed delete cancels nothing.
        s.observe(
            &call("d3", "CronDelete", serde_json::json!({ "id": "e5f6a7b8" })),
            t0,
        );
        s.observe(&result("d3", true, "no such job"), t0);
        assert!(s.pending(t0).is_some());
        s.observe(
            &call("d4", "CronDelete", serde_json::json!({ "id": "e5f6a7b8" })),
            t0,
        );
        s.observe(&result("d4", false, "Cancelled e5f6a7b8"), t0);
        assert_eq!(s.pending(t0), None);
    }

    #[test]
    fn an_id_is_named_only_as_a_whole_word() {
        assert!(names("job abc123 (hourly)", "abc123"));
        assert!(names("abc123", "abc123"));
        assert!(!names("job abc1234", "abc123"));
        assert!(!names("job xabc123", "abc123"));
        assert!(!names("anything", ""));
    }
}
