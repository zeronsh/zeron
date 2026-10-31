//! Claude's session-scoped scheduled work: `ScheduleWakeup` and `CronCreate`.
//!
//! The CLI keeps these timers in its own process and does not list them in
//! `background_tasks_changed` (2.1.316: only shells, monitors, agents and
//! workflows are tasks). Retiring the runtime before a timer fires silently
//! drops the wakeup, so the host must be told to hold it. The tool calls and
//! their results are the only signal on the wire:
//!
//! - `ScheduleWakeup {delaySeconds, stop}` — one wakeup after the delay, which
//!   the CLI clamps to 60..=3600 s; `stop: true` cancels the pending ones;
//! - `CronCreate {cron, recurring}` → "Scheduled recurring job <id> …" (kept
//!   until `CronDelete`, and at most seven days: the CLI's own expiry) or
//!   "Scheduled one-shot task <id> …" (until its fire time, which the cron
//!   expression pins);
//! - `CronDelete {id}` → "Cancelled job <id>."; `CronList` → "No scheduled
//!   jobs." when none remain.
//!
//! Durable jobs persist in the project's `.claude/scheduled_tasks.json` and
//! are not re-detected after a resume; only jobs seen on this process's
//! wire hold it.

use std::collections::HashMap;

use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc};
use serde_json::Value;

use super::wire::Frame;

const WAKEUP_MIN_S: u64 = 60;
const WAKEUP_MAX_S: u64 = 3600;
/// The wakeup fires "earlier or later" than its time; and a one-shot job is
/// jittered by up to 90 s.
const FIRE_GRACE_MS: u64 = 15 * 60 * 1000;
/// The CLI auto-expires a recurring job after seven days.
const RECURRING_MAX_AGE_MS: u64 = 7 * 24 * 60 * 60 * 1000;

enum Call {
    Wakeup { stop: bool, delay_s: u64 },
    Create { cron: String, recurring: bool },
    Delete,
    List,
}

#[derive(Default)]
pub(super) struct ScheduledWork {
    calls: HashMap<String, Call>,
    wakeup_until: u64,
    /// Job id → unix ms until which it holds the runtime.
    jobs: HashMap<String, u64>,
}

impl ScheduledWork {
    /// The unix-ms time until which the runtime must stay (0 = nothing).
    pub fn hold_until(&self) -> u64 {
        self.jobs
            .values()
            .copied()
            .chain([self.wakeup_until])
            .max()
            .unwrap_or(0)
    }

    /// Take one frame in. True when the hold may have changed.
    pub fn observe(&mut self, frame: &Frame, now_ms: u64) -> bool {
        match frame {
            Frame::Assistant(message) => {
                for block in message.message.blocks() {
                    if block.kind != "tool_use" {
                        continue;
                    }
                    let call = match block.name.as_str() {
                        "ScheduleWakeup" => Call::Wakeup {
                            stop: block.input.get("stop").and_then(Value::as_bool) == Some(true),
                            delay_s: block
                                .input
                                .get("delaySeconds")
                                .and_then(Value::as_u64)
                                .unwrap_or(WAKEUP_MIN_S)
                                .clamp(WAKEUP_MIN_S, WAKEUP_MAX_S),
                        },
                        "CronCreate" => Call::Create {
                            cron: block
                                .input
                                .get("cron")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            recurring: block.input.get("recurring").and_then(Value::as_bool)
                                != Some(false),
                        },
                        "CronDelete" => Call::Delete,
                        "CronList" => Call::List,
                        _ => continue,
                    };
                    self.calls.insert(block.id, call);
                }
                false
            }
            Frame::User(message) => {
                let Some(blocks) = message.message.content.as_array() else {
                    return false;
                };
                let mut changed = false;
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let Some(call) = block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .and_then(|id| self.calls.remove(id).map(|call| (id.to_owned(), call)))
                    else {
                        continue;
                    };
                    if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                        continue;
                    }
                    let text = result_text(block.get("content"));
                    changed |= self.apply(call, &text, now_ms);
                }
                changed
            }
            _ => false,
        }
    }

    fn apply(&mut self, (tool, call): (String, Call), text: &str, now_ms: u64) -> bool {
        match call {
            Call::Wakeup { stop: true, .. } => {
                self.wakeup_until = 0;
                true
            }
            Call::Wakeup { delay_s, .. } => {
                self.wakeup_until = self
                    .wakeup_until
                    .max(now_ms + delay_s * 1000 + FIRE_GRACE_MS);
                true
            }
            Call::Create { cron, recurring } => {
                let id = ["Scheduled recurring job ", "Scheduled one-shot task "]
                    .iter()
                    .find_map(|marker| text.split_once(marker))
                    .and_then(|(_, rest)| rest.split_whitespace().next())
                    .map(str::to_owned);
                // A result that names no job still scheduled something if it
                // says so; anything else created nothing.
                if id.is_none() && !text.contains("Scheduled ") {
                    return false;
                }
                let until = if recurring {
                    now_ms + RECURRING_MAX_AGE_MS
                } else {
                    // Unparsable: no fire time is known, so assume the longest.
                    next_fire_ms(&cron, now_ms)
                        .map_or(now_ms + RECURRING_MAX_AGE_MS, |at| at + FIRE_GRACE_MS)
                };
                self.jobs.insert(id.unwrap_or(tool), until);
                true
            }
            Call::Delete => {
                let Some(id) = text
                    .split_once("Cancelled job ")
                    .and_then(|(_, rest)| rest.split_whitespace().next())
                    .map(|id| id.trim_end_matches('.'))
                else {
                    return false;
                };
                self.jobs.remove(id).is_some()
            }
            Call::List => {
                if text.contains("No scheduled jobs") && !self.jobs.is_empty() {
                    self.jobs.clear();
                    return true;
                }
                false
            }
        }
    }
}

fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// One cron field as the set of values it matches.
fn field(spec: &str, lo: u32, hi: u32) -> Option<Vec<bool>> {
    let mut set = vec![false; (hi + 1) as usize];
    for part in spec.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (range, step.parse::<u32>().ok().filter(|s| *s > 0)?),
            None => (part, 1),
        };
        let (from, to) = if range == "*" {
            (lo, hi)
        } else if let Some((a, b)) = range.split_once('-') {
            (a.parse().ok()?, b.parse().ok()?)
        } else {
            let v = range.parse().ok()?;
            // `5/10` runs from 5 to the end; a bare value is itself.
            (v, if part.contains('/') { hi } else { v })
        };
        if from < lo || to > hi || from > to {
            return None;
        }
        (from..=to).step_by(step as usize).for_each(|v| {
            set[v as usize] = true;
        });
    }
    Some(set)
}

/// The next time after `after_ms` a five-field cron expression fires, as the
/// later of its local-time and UTC readings (the CLI's zone is not on the
/// wire, and holding too long is the safe error).
fn next_fire_ms(cron: &str, after_ms: u64) -> Option<u64> {
    let fields: Vec<&str> = cron.split_whitespace().collect();
    let [minute, hour, dom, month, dow] = fields.as_slice() else {
        return None;
    };
    let minute_set = field(minute, 0, 59)?;
    let hour_set = field(hour, 0, 23)?;
    let dom_set = field(dom, 1, 31)?;
    let month_set = field(month, 1, 12)?;
    let mut dow_set = field(dow, 0, 7)?;
    dow_set[0] |= dow_set[7];
    let dom_any = *dom == "*";
    let dow_any = *dow == "*";
    let day_matches = |date: NaiveDate| {
        let in_dom = dom_set[date.day() as usize];
        let in_dow = dow_set[date.weekday().num_days_from_sunday() as usize];
        month_set[date.month() as usize]
            && match (dom_any, dow_any) {
                (false, false) => in_dom || in_dow,
                _ => in_dom && in_dow,
            }
    };
    let after = |naive: NaiveDateTime| naive + chrono::Duration::minutes(1);
    let search = |start: NaiveDateTime| -> Option<NaiveDateTime> {
        let start = after(start.with_second(0)?.with_nanosecond(0)?);
        for offset in 0..=366 {
            let date = start.date() + chrono::Duration::days(offset);
            if !day_matches(date) {
                continue;
            }
            for h in 0..24u32 {
                for m in 0..60u32 {
                    if !hour_set[h as usize] || !minute_set[m as usize] {
                        continue;
                    }
                    let at = date.and_hms_opt(h, m, 0)?;
                    if at >= start {
                        return Some(at);
                    }
                }
            }
        }
        None
    };
    let now_utc = Utc.timestamp_millis_opt(after_ms as i64).single()?;
    let utc = search(now_utc.naive_utc())
        .map(|at| Utc.from_utc_datetime(&at).timestamp_millis())
        .into_iter();
    let local = search(now_utc.with_timezone(&Local).naive_local())
        .and_then(|at| Local.from_local_datetime(&at).latest())
        .map(|at| at.timestamp_millis())
        .into_iter();
    utc.chain(local).max().map(|ms| ms.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::wire::parse_frame;

    const NOW: u64 = 1_800_000_000_000;

    fn call(id: &str, name: &str, input: &str) -> Frame {
        parse_frame(&format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}]}}}}"#
        ))
        .expect("parses")
    }
    fn result(id: &str, text: &str, error: bool) -> Frame {
        parse_frame(&format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"tool_use_id":"{id}","type":"tool_result","content":{text:?},"is_error":{error}}}]}}}}"#
        ))
        .expect("parses")
    }
    fn run(work: &mut ScheduledWork, frames: &[Frame], now: u64) -> bool {
        frames.iter().fold(false, |c, f| c | work.observe(f, now))
    }

    #[test]
    fn a_wakeup_holds_its_delay_clamped_plus_grace_and_stop_releases_it() {
        let mut work = ScheduledWork::default();
        assert_eq!(work.hold_until(), 0);
        assert!(run(
            &mut work,
            &[
                call(
                    "w1",
                    "ScheduleWakeup",
                    r#"{"delaySeconds":270,"reason":"r","prompt":"p"}"#
                ),
                result("w1", "ok", false),
            ],
            NOW
        ));
        assert_eq!(work.hold_until(), NOW + 270_000 + FIRE_GRACE_MS);
        // Beyond the CLI's clamp, the clamp applies.
        run(
            &mut work,
            &[
                call(
                    "w2",
                    "ScheduleWakeup",
                    r#"{"delaySeconds":99999,"reason":"r","prompt":"p"}"#,
                ),
                result("w2", "ok", false),
            ],
            NOW,
        );
        assert_eq!(work.hold_until(), NOW + 3_600_000 + FIRE_GRACE_MS);
        assert!(run(
            &mut work,
            &[
                call("w3", "ScheduleWakeup", r#"{"stop":true}"#),
                result("w3", "ok", false),
            ],
            NOW
        ));
        assert_eq!(work.hold_until(), 0);
    }

    #[test]
    fn a_refused_call_schedules_nothing() {
        let mut work = ScheduledWork::default();
        run(
            &mut work,
            &[
                call("w1", "ScheduleWakeup", r#"{"delaySeconds":120}"#),
                result("w1", "ScheduleWakeup refused", true),
                call("c1", "CronCreate", r#"{"cron":"*/5 * * * *","prompt":"p"}"#),
                result("c1", "Invalid cron expression", true),
            ],
            NOW,
        );
        assert_eq!(work.hold_until(), 0);
    }

    #[test]
    fn a_recurring_cron_holds_until_deleted_or_the_cli_expires_it() {
        let mut work = ScheduledWork::default();
        run(
            &mut work,
            &[
                call("c1", "CronCreate", r#"{"cron":"*/5 * * * *","prompt":"p"}"#),
                result(
                    "c1",
                    "Scheduled recurring job abc123 (every 5 minutes). Session-only (not written to disk, dies when Claude exits). Auto-expires after 7 days. Use CronDelete to cancel sooner.",
                    false,
                ),
            ],
            NOW,
        );
        assert_eq!(work.hold_until(), NOW + RECURRING_MAX_AGE_MS);
        // Deleting an unknown job changes nothing.
        assert!(!run(
            &mut work,
            &[
                call("d0", "CronDelete", r#"{"id":"zzz"}"#),
                result("d0", "Cancelled job zzz.", false),
            ],
            NOW
        ));
        assert!(run(
            &mut work,
            &[
                call("d1", "CronDelete", r#"{"id":"abc123"}"#),
                result("d1", "Cancelled job abc123.", false),
            ],
            NOW
        ));
        assert_eq!(work.hold_until(), 0);
    }

    #[test]
    fn an_empty_cron_list_clears_every_job() {
        let mut work = ScheduledWork::default();
        run(
            &mut work,
            &[
                call("c1", "CronCreate", r#"{"cron":"0 9 * * *","prompt":"p"}"#),
                result("c1", "Scheduled recurring job j1 (daily).", false),
                call("l1", "CronList", "{}"),
                result("l1", "j1 (recurring) 0 9 * * *", false),
            ],
            NOW,
        );
        assert!(work.hold_until() > 0, "a listed job stays");
        run(
            &mut work,
            &[
                call("l2", "CronList", "{}"),
                result("l2", "No scheduled jobs.", false),
            ],
            NOW,
        );
        assert_eq!(work.hold_until(), 0);
    }

    #[test]
    fn a_one_shot_holds_until_its_pinned_fire_time_not_forever() {
        let mut work = ScheduledWork::default();
        // 2027-01-15 08:00 UTC; the job pins Jan 20 at 09:30.
        let now = 1_799_999_999_000;
        run(
            &mut work,
            &[
                call(
                    "c1",
                    "CronCreate",
                    r#"{"cron":"30 9 20 1 *","prompt":"p","recurring":false}"#,
                ),
                result(
                    "c1",
                    "Scheduled one-shot task t1 (Jan 20 at 9:30). It will fire once then auto-delete.",
                    false,
                ),
            ],
            now,
        );
        let hold = work.hold_until();
        let fire = next_fire_ms("30 9 20 1 *", now).unwrap();
        assert_eq!(hold, fire + FIRE_GRACE_MS);
        assert!(hold > now && hold < now + 400 * 24 * 3600 * 1000);
        // Five days out, not seven: it does not fall back to the expiry cap.
        assert!(hold < now + 8 * 24 * 3600 * 1000 + 24 * 3600 * 1000);
    }

    #[test]
    fn an_unparsable_one_shot_holds_the_longest() {
        let mut work = ScheduledWork::default();
        run(
            &mut work,
            &[
                call(
                    "c1",
                    "CronCreate",
                    r#"{"cron":"nonsense","recurring":false}"#,
                ),
                result("c1", "Scheduled one-shot task t1 (?).", false),
            ],
            NOW,
        );
        assert_eq!(work.hold_until(), NOW + RECURRING_MAX_AGE_MS);
    }

    #[test]
    fn cron_fields_parse_lists_ranges_and_steps() {
        let set = |s: &str| {
            field(s, 0, 59)
                .unwrap()
                .iter()
                .enumerate()
                .filter_map(|(i, on)| on.then_some(i))
                .collect::<Vec<_>>()
        };
        assert_eq!(set("*/20"), [0, 20, 40]);
        assert_eq!(set("1,5-7"), [1, 5, 6, 7]);
        assert_eq!(set("50/5"), [50, 55]);
        assert_eq!(set("10-20/5"), [10, 15, 20]);
        assert!(field("61", 0, 59).is_none());
        assert!(field("*/0", 0, 59).is_none());
        assert!(field("9-3", 0, 59).is_none());
        assert!(next_fire_ms("* * * *", NOW).is_none());
        assert!(next_fire_ms("0 0 31 2 *", NOW).is_none(), "no 31 February");
    }
}
