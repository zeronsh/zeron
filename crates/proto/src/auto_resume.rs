//! Auto-resume: a turn the provider's usage limit stopped continues by itself
//! once the limit resets. The chat's host decides and acts; every device reads
//! the stop's state off the chat's session row ([`crate::Session::auto_resume`])
//! and flips it with `SetChatAutoResume`. Whether new stops resume by default
//! is a device setting (`GetAutoResume` / `SetAutoResume`).

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

/// A turn the provider's usage limit stopped, and what the host does about it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoResume {
    /// The assistant entry the limit stopped: the transcript entry whose
    /// error card shows this state.
    pub entry_id: String,
    /// Resume this stop once the limit resets. Starts as the host's device
    /// setting; the error card's toggle flips it (`SetChatAutoResume`).
    pub enabled: bool,
    pub phase: AutoResumePhase,
    /// When the host next checks the limit and, if it has reset, resumes:
    /// the reset the provider or the account's usage gave, pushed later when
    /// resumes keep running straight back into the limit. `None` while it
    /// doesn't know ("Waiting for the limit to reset…"); it still checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_at: Option<DateTime<Utc>>,
    /// When the limit stopped the turn.
    pub stopped_at: DateTime<Utc>,
    /// When the continuation started (phase `Resumed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_at: Option<DateTime<Utc>>,
    /// How many times in a row this chat's work was resumed and stopped by
    /// the limit again (or failed to start); spaces out the checks, and past
    /// [`MAX_ATTEMPTS`] the host gives up (`GaveUp`).
    #[serde(default)]
    pub attempt: u32,
}

/// Where an [`AutoResume`] stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoResumePhase {
    /// Waiting for the limit to reset (or, when not `enabled`, for the user).
    Waiting,
    /// The limit has reset and the continuation is being started.
    Resuming,
    /// The continuation turn started.
    Resumed,
    /// The host stopped trying: resumes kept running straight back into the
    /// limit ("Auto-resume stopped — try again manually"). Switching the
    /// stop's toggle on again starts over.
    GaveUp,
    /// A phase a newer host wrote that this build doesn't know.
    #[serde(other)]
    Unknown,
}

/// How many resumes in a row may run straight back into the limit before
/// the host gives up on a stop.
pub const MAX_ATTEMPTS: u32 = 8;

/// How long after a stop the host checks again when it knows no reset time
/// — and the least it waits after a resume ran straight back into the limit
/// — growing with each attempt in a row.
pub fn check_backoff(attempt: u32) -> Duration {
    const CHECK_BACKOFF: [Duration; 4] = [
        Duration::minutes(5),
        Duration::minutes(15),
        Duration::minutes(30),
        Duration::minutes(60),
    ];
    CHECK_BACKOFF[(attempt as usize).min(CHECK_BACKOFF.len() - 1)]
}

impl AutoResume {
    /// The stop will resume by itself: enabled and not resumed yet. A parent
    /// waiting on this chat's turn keeps waiting through it.
    pub fn pending(&self) -> bool {
        self.enabled
            && matches!(
                self.phase,
                AutoResumePhase::Waiting | AutoResumePhase::Resuming
            )
    }
}

/// The device setting (`GetAutoResume` / `SetAutoResume`): whether a turn
/// the usage limit stops on this device resumes by itself once the limit
/// resets. On unless the user turned it off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoResumeSettings {
    pub enabled: bool,
}

impl Default for AutoResumeSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Whether `message` reads as a usage limit that resets by itself — the
/// account's own plan window — for harnesses that give no structured signal
/// (see [`crate::AgentEvent::LimitReached`]). Stricter than
/// [`crate::run_errors::provider_problem`]: it takes "usage limit" /
/// "you've hit your"-style wording, and none of what reads like a limit but
/// isn't one a reset clears ([`limit_never_resets`]) or is a short throttle
/// (per-minute rates, "try again in 20s"), a cap on turns, tool calls or
/// output, a session limit, a free tier or an upgrade prompt.
pub fn limit_stop_text(message: &str) -> bool {
    const STOPS: &[&str] = &[
        "you've hit your",
        "you have hit your",
        "usage limit reached",
        "usage limit has been reached",
        "usage limit exceeded",
        "hit your limit",
        "hit your usage limit",
        "hour limit reached",
        "weekly limit reached",
        "daily limit reached",
        "quota will reset",
    ];
    const NOT: &[&str] = &[
        "context",
        "upgrade",
        "per min",
        "per second",
        "turns",
        "tool call",
        "recursion",
        "output token",
        "session limit",
        "free tier",
        "spend cap",
        "spend limit",
        "out of credits",
        "out of usage credits",
        "usage credit limit",
    ];
    let text = normalize(message);
    STOPS.iter().any(|stop| text.contains(stop))
        && !NOT.iter().any(|not| text.contains(not))
        && !text
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| matches!(word, "tpm" | "rpm" | "rps"))
        && !short_wait(&text)
        && !never_resets(&text)
}

/// Whether `message` reports a limit no reset clears, whatever its wording
/// otherwise: a server throttle ("not your usage limit"), a model under
/// load, an organization or seat that has the service turned off, an admin
/// to ask, billing quota, a plan without the service, a credit balance or
/// spend cap that says no time it resets. Applied to every harness's stop,
/// structured signal or not.
pub fn limit_never_resets(message: &str, now: DateTime<Utc>) -> bool {
    const SPENT: &[&str] = &[
        "spend cap",
        "spend limit",
        "out of credits",
        "out of usage credits",
        "usage credit limit",
    ];
    let text = normalize(message);
    never_resets(&text)
        || (SPENT.iter().any(|spent| text.contains(spent)) && reset_time_in(message, now).is_none())
}

fn never_resets(text: &str) -> bool {
    const NEVER: &[&str] = &[
        "not your usage limit",
        "temporarily limiting",
        "high load",
        "overloaded",
        "contact your admin",
        "disabled",
        "seat type",
        "set to $0",
        "quota exceeded",
        "upgrade to plus",
        "credits balance",
        "free tier",
    ];
    NEVER.iter().any(|never| text.contains(never))
}

/// A wait of under two minutes ("try again in 20s"): a rate throttle, not a
/// usage window.
fn short_wait(text: &str) -> bool {
    ["try again in ", "retry in ", "retry after ", "resets in "]
        .iter()
        .any(|marker| {
            text.match_indices(marker).any(|(at, _)| {
                parse_wait(&text[at + marker.len()..])
                    .is_some_and(|wait| wait < Duration::minutes(2))
            })
        })
}

/// Lowercase, with the typographic marks providers use made plain.
fn normalize(message: &str) -> String {
    message
        .to_lowercase()
        .replace(['\u{2019}', '\u{2018}'], "'")
        .replace(['\u{a0}', '\u{202f}'], " ")
}

/// When the limit that `message` reports resets, if the text says, as an
/// instant at most a week and a day after `now`:
/// - a wait: Codex's "it resets in 2h 14m", "try again in 20 minutes",
///   "retry after 30 seconds";
/// - Claude's older "…limit reached|1767225600" (Unix seconds or millis);
/// - a wall time: Claude's "resets 3pm (Europe/Berlin)", "resets Nov 3,
///   10am (UTC)", "resets May 5 at 9pm"; Codex's "try again at 3:40 PM" or
///   "try again at Feb 23rd, 2026 9:01 PM"; Hermes' "limit resets at 14:05
///   (in 1h 5m)". A time is read in the zone it names when that is this
///   device's or UTC, and in this device's zone when it names none; another
///   zone can't be resolved here (`None`). A time without a date is its next
///   occurrence after `now` — except Codex's ("try again at"), which leaves
///   the date out only for today: one just past reads as `now`.
pub fn reset_time_in(message: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let text = normalize(message);
    let found = epoch_in(&text)
        .or_else(|| {
            [
                "resets in ",
                "reset in ",
                "try again in ",
                "retry in ",
                "retry after ",
            ]
            .iter()
            .find_map(|marker| {
                let at = text.find(marker)?;
                parse_wait(&text[at + marker.len()..]).map(|wait| now + wait)
            })
        })
        .or_else(|| {
            ["resets at ", "resets ", "reset at ", "try again at "]
                .iter()
                .find_map(|marker| {
                    text.match_indices(marker).find_map(|(at, _)| {
                        let today_only = *marker == "try again at ";
                        parse_wall_time(&text[at + marker.len()..], now, today_only)
                    })
                })
        })?;
    // A plan window resets within a week; anything else is a misread.
    (found > now - Duration::minutes(1) && found < now + Duration::days(8)).then_some(found)
}

/// "…|1767225600" (seconds) or "…|1767225600000" (millis).
fn epoch_in(text: &str) -> Option<DateTime<Utc>> {
    let (_, tail) = text.rsplit_once('|')?;
    let digits: String = tail
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    match digits.len() {
        10 => DateTime::from_timestamp(digits.parse().ok()?, 0),
        13 => DateTime::from_timestamp_millis(digits.parse().ok()?),
        _ => None,
    }
}

/// "2h 14m", "45m", "3d 2h", "20 minutes", "1m30s", "30 seconds".
fn parse_wait(text: &str) -> Option<Duration> {
    let mut total = Duration::zero();
    let mut seen = false;
    let mut rest = text.trim_start();
    loop {
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if digits == 0 {
            break;
        }
        let Ok(value) = rest[..digits].parse::<i64>() else {
            break;
        };
        let after = rest[digits..].trim_start();
        let unit_len = after
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(after.len());
        let unit = &after[..unit_len];
        let step = match unit {
            "d" | "day" | "days" => Duration::days(value),
            "h" | "hr" | "hrs" | "hour" | "hours" => Duration::hours(value),
            "m" | "min" | "mins" | "minute" | "minutes" => Duration::minutes(value),
            "s" | "sec" | "secs" | "second" | "seconds" => Duration::seconds(value),
            _ => break,
        };
        total += step;
        seen = true;
        rest = after[unit_len..].trim_start_matches([' ', ',']);
        rest = rest.strip_prefix("and ").unwrap_or(rest);
    }
    seen.then_some(total)
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// The zone a wall time is read in.
#[derive(Clone, Copy)]
enum Zone {
    Local,
    Utc,
}

/// The digits leading `text`, at most `max` of them, and the rest.
fn number(text: &str, max: usize) -> Option<(u32, &str)> {
    let digits = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    if digits == 0 || digits > max {
        return None;
    }
    Some((text[..digits].parse().ok()?, &text[digits..]))
}

/// "3pm", "3:40 pm", "15:40", "nov 3, 10am", "may 5 at 9pm",
/// "feb 23rd, 2026 9:01 pm", each optionally followed by "(zone)".
/// `today_only`: a time without a date is today's, even when just past.
fn parse_wall_time(text: &str, now: DateTime<Utc>, today_only: bool) -> Option<DateTime<Utc>> {
    let mut rest = text.trim_start();
    let mut date = None;
    if let Some(month) = MONTHS.iter().position(|m| rest.starts_with(m)) {
        let after = rest
            .trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == '.')
            .trim_start();
        let (day, after) = number(after, 2)?;
        let after = ["st", "nd", "rd", "th"]
            .iter()
            .find_map(|suffix| after.strip_prefix(suffix))
            .unwrap_or(after);
        let mut year = None;
        rest = after.trim_start_matches([',', ' ']);
        if let Some((y, after)) = number(rest, 4).filter(|(y, after)| {
            *y >= 2000 && !after.starts_with(':') && !after.trim_start().starts_with(['a', 'p'])
        }) {
            year = Some(y as i32);
            rest = after.trim_start_matches([',', ' ']);
        }
        rest = rest.strip_prefix("at ").unwrap_or(rest);
        date = Some((year, month as u32 + 1, day));
    }
    let (mut hour, after) = number(rest, 2)?;
    rest = after;
    let mut minute = 0;
    let colon = rest.starts_with(':');
    if let Some(after) = rest.strip_prefix(':') {
        let digits = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        if digits != 2 {
            return None;
        }
        minute = after[..digits].parse().ok()?;
        rest = &after[digits..];
    }
    rest = rest.trim_start();
    let meridiem = if let Some(after) = rest.strip_prefix("am").or(rest.strip_prefix("a.m.")) {
        rest = after;
        Some(false)
    } else if let Some(after) = rest.strip_prefix("pm").or(rest.strip_prefix("p.m.")) {
        rest = after;
        Some(true)
    } else {
        None
    };
    match meridiem {
        Some(pm) => {
            if !(1..=12).contains(&hour) {
                return None;
            }
            hour = hour % 12 + if pm { 12 } else { 0 };
        }
        // A bare number ("resets 5 times") is not a time; "15:40" is.
        None if !colon || hour > 23 => return None,
        None => {}
    }
    // "(Europe/Berlin)", "(UTC)" — not "(in 1h 5m)".
    let zone = match rest
        .trim_start()
        .strip_prefix('(')
        .and_then(|z| z.split_once(')'))
        .filter(|(name, _)| !name.trim().contains(' '))
    {
        None => Zone::Local,
        Some((name, _)) => match name.trim() {
            "utc" | "gmt" | "etc/utc" | "etc/gmt" | "z" => Zone::Utc,
            name if iana_time_zone::get_timezone()
                .is_ok_and(|local| local.eq_ignore_ascii_case(name)) =>
            {
                Zone::Local
            }
            _ => return None,
        },
    };
    let time = NaiveTime::from_hms_opt(hour, minute, 0)?;
    let at = |year: i32, month: u32, day: u32| {
        let naive = chrono::NaiveDate::from_ymd_opt(year, month, day)?.and_time(time);
        match zone {
            Zone::Utc => Some(Utc.from_utc_datetime(&naive)),
            // Ambiguous across a DST change: the later one is never early.
            Zone::Local => Local
                .from_local_datetime(&naive)
                .latest()
                .map(|t| t.with_timezone(&Utc)),
        }
    };
    let today = match zone {
        Zone::Utc => now.date_naive(),
        Zone::Local => now.with_timezone(&Local).date_naive(),
    };
    match date {
        Some((Some(year), month, day)) => at(year, month, day),
        Some((None, month, day)) => {
            let this_year = at(today.year(), month, day)?;
            // "Jan 2" read on Dec 30 is next year's.
            if this_year < now - Duration::days(2) {
                at(today.year() + 1, month, day)
            } else {
                Some(this_year)
            }
        }
        None => {
            let same_day = at(today.year(), today.month(), today.day())?;
            if same_day > now - Duration::minutes(1) {
                Some(same_day)
            } else if today_only {
                Some(now)
            } else {
                // The next day's wall time (a DST change makes that not
                // exactly 24 hours on).
                let tomorrow = today.succ_opt()?;
                at(tomorrow.year(), tomorrow.month(), tomorrow.day())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339).unwrap().into()
    }

    #[test]
    fn reads_relative_waits() {
        let now = at("2026-11-01T12:00:00Z");
        for (text, wait) in [
            (
                "Codex usage limit reached — it resets in 2h 14m.",
                Duration::minutes(134),
            ),
            (
                "Codex rate limit reached — it resets in 45m.",
                Duration::minutes(45),
            ),
            (
                "Codex usage limit reached — it resets in 3d 2h.",
                Duration::hours(74),
            ),
            (
                "Rate limit reached. Please try again in 1m30s.",
                Duration::seconds(90),
            ),
            ("429: retry after 30 seconds", Duration::seconds(30)),
            (
                "Hermes: its usage limit resets in 4h 5m. Send /retry after that",
                Duration::minutes(245),
            ),
        ] {
            assert_eq!(reset_time_in(text, now), Some(now + wait), "{text}");
        }
    }

    /// `h:m` on `now`'s local day, or the next day when that has passed.
    fn local(now: DateTime<Utc>, h: u32, m: u32) -> DateTime<Utc> {
        let today = now.with_timezone(&Local).date_naive();
        let t = Local
            .from_local_datetime(&today.and_hms_opt(h, m, 0).unwrap())
            .latest()
            .unwrap()
            .with_timezone(&Utc);
        if t > now { t } else { t + Duration::days(1) }
    }

    #[test]
    fn reads_unix_and_wall_times() {
        let now = at("2026-11-01T12:00:00Z");
        assert_eq!(
            reset_time_in("Claude AI usage limit reached|1793620800", now),
            DateTime::from_timestamp(1_793_620_800, 0)
        );
        assert_eq!(
            reset_time_in("Claude AI usage limit reached|1793620800000", now),
            DateTime::from_timestamp(1_793_620_800, 0)
        );
        assert_eq!(
            reset_time_in("5-hour limit reached ∙ resets 3pm", now),
            Some(local(now, 15, 0))
        );
        assert_eq!(
            reset_time_in(
                "You’ve hit your usage limit. Upgrade to Pro or try again at 3:40 PM.",
                now
            ),
            Some(local(now, 15, 40))
        );
        assert_eq!(
            reset_time_in("API Error: 429 rate_limit_error (resets 11am)", now),
            Some(local(now, 11, 0))
        );
        let dated = reset_time_in("You've hit your weekly limit · resets Nov 3, 10am", now)
            .unwrap()
            .with_timezone(&Local);
        assert_eq!((dated.month(), dated.day(), dated.hour()), (11, 3, 10));
        let dated = reset_time_in(
            "You've hit your limit · resets May 5 at 9pm",
            at("2026-05-01T12:00:00Z"),
        )
        .unwrap()
        .with_timezone(&Local);
        assert_eq!((dated.month(), dated.day(), dated.hour()), (5, 5, 21));
        let dated = reset_time_in("try again at Nov 6th, 2026 9:01 PM.", now)
            .unwrap()
            .with_timezone(&Local);
        assert_eq!(
            (
                dated.year(),
                dated.month(),
                dated.day(),
                dated.hour(),
                dated.minute()
            ),
            (2026, 11, 6, 21, 1)
        );
    }

    #[test]
    fn reads_a_named_zone_only_when_it_can() {
        let now = at("2026-11-01T12:00:00Z");
        assert_eq!(
            reset_time_in("You've hit your 5-hour limit · resets 3pm (UTC)", now),
            Some(at("2026-11-01T15:00:00Z"))
        );
        assert_eq!(
            reset_time_in(
                "You've hit your weekly limit · resets Nov 3, 10:30am (UTC)",
                now
            ),
            Some(at("2026-11-03T10:30:00Z"))
        );
        // Past today in UTC: tomorrow's.
        assert_eq!(
            reset_time_in("You've hit your limit · resets 11am (UTC)", now),
            Some(at("2026-11-02T11:00:00Z"))
        );
        if let Ok(zone) = iana_time_zone::get_timezone() {
            assert_eq!(
                reset_time_in(&format!("You've hit your limit · resets 3pm ({zone})"), now),
                Some(local(now, 15, 0))
            );
        }
        // A zone this device can't resolve: unknown, not guessed.
        assert_eq!(
            reset_time_in(
                "You've hit your limit · resets 3pm (Mars/Olympus_Mons)",
                now
            ),
            None
        );
    }

    #[test]
    fn ignores_text_without_a_plausible_time() {
        let now = at("2026-11-01T12:00:00Z");
        for text in [
            "Claude usage limit reached — try again after the limit resets.",
            "Claude 5-hour limit reached — the turn was blocked. Try again after it resets.",
            "429 Too Many Requests",
            "The counter resets 5 times a day",
            // Further out than any plan window.
            "Usage resets in 40d",
            "Your usage limits will reset when your monthly cycle ends on 8/14/2025.",
        ] {
            assert_eq!(reset_time_in(text, now), None, "{text}");
        }
    }

    #[test]
    fn tells_a_resetting_limit_from_its_look_alikes() {
        for stop in [
            "You've hit your 5-hour limit · resets 3pm (Europe/Berlin)",
            "You’ve hit your weekly limit · resets Nov 3, 10am",
            "Claude AI usage limit reached|1753441200",
            "5-hour limit reached ∙ resets 5am",
            "Gemini quota will reset after 5h.",
            "Hermes: its usage limit has been reached. Limit resets at 14:05 (in 1h 5m).",
        ] {
            assert!(limit_stop_text(stop), "{stop}");
        }
        for other in [
            "API Error: Server is temporarily limiting requests (not your usage limit) · this may be a temporary capacity issue.",
            "API Error: Request rejected (429) · rate limited",
            "Opus is experiencing high load, please use /model to switch to Sonnet",
            "Quota exceeded. Check your plan and billing details.",
            "To use Codex with your ChatGPT plan, upgrade to Plus: https://chatgpt.com/explore/plus",
            "Your workspace is out of credits.",
            "You hit your spend cap set by your admin.",
            "You've hit your usage limit · contact your admin to increase it",
            "Claude is overloaded right now — try again shortly.",
            "Harness exited with status 1",
            // Wording that only looks like a usage window.
            "Rate limit reached for gpt-5 in organization org-x on tokens per min (TPM): Limit 30000.",
            "Usage limit reached: 60 RPM. Please try again in 20s.",
            "Usage limit reached — try again in 30 seconds",
            "You've hit your limit of 50 turns for this task",
            "Usage limit reached: too many tool calls in one turn",
            "Recursion limit reached",
            "Usage limit reached for output tokens",
            "You've hit your session limit for this conversation",
            "You've hit your free tier usage limit",
            "You've hit your usage limit. Upgrade to Pro to keep going.",
            "Context limit reached",
            "Max turns limit reached",
        ] {
            assert!(!limit_stop_text(other), "{other}");
        }
    }

    #[test]
    fn some_limits_never_reset_whatever_signalled_them() {
        let now = at("2026-11-01T12:00:00Z");
        for never in [
            "API Error: Server is temporarily limiting requests (not your usage limit)",
            "You've hit your usage limit · contact your admin to increase it",
            "Your seat type doesn't include usage credits",
            "This service is disabled for your org",
            "Quota exceeded. Check your plan and billing details.",
            "You've hit your monthly spend limit · raise it in settings",
            "You're out of usage credits",
        ] {
            assert!(limit_never_resets(never, now), "{never}");
        }
        for resets in [
            "You've hit your 5-hour limit · resets 3pm (UTC)",
            // A spend cap that says when it resets does reset.
            "You've hit your monthly spend limit · your 5-hour limit resets 3pm (UTC)",
            "You're out of usage credits · resets Nov 3, 10am (UTC)",
            "You’ve hit your usage limit. Upgrade to Pro or try again at 3:40 PM.",
        ] {
            assert!(!limit_never_resets(resets, now), "{resets}");
        }
    }

    #[test]
    fn codex_leaves_the_date_out_only_for_today() {
        // Local noon, so three minutes back stays on the same local day.
        let now = Local
            .with_ymd_and_hms(2026, 11, 1, 12, 0, 0)
            .latest()
            .unwrap()
            .with_timezone(&Utc);
        let local_now = now.with_timezone(&Local);
        let just_past = (local_now - Duration::minutes(3)).format("%-I:%M %p");
        // Codex: today's, just past: already reset.
        assert_eq!(
            reset_time_in(
                &format!("You’ve hit your usage limit. Try again at {just_past}."),
                now
            ),
            Some(now)
        );
        // Claude: the next one.
        let claude =
            reset_time_in(&format!("You've hit your limit · resets {just_past}"), now).unwrap();
        assert!(claude > now + Duration::hours(23), "{claude}");
    }

    #[test]
    fn reads_hermes_wall_time_without_taking_its_wait_for_a_zone() {
        let now = at("2026-11-01T12:00:00Z");
        assert_eq!(
            reset_time_in("Limit resets at 14:05 (in 1h 5m).", now),
            Some(local(now, 14, 5))
        );
    }

    #[test]
    fn a_newer_phase_reads_as_unknown_rather_than_failing_the_row() {
        let stop: AutoResume = serde_json::from_value(serde_json::json!({
            "entryId": "a", "enabled": true, "phase": "pausedForReasons",
            "stoppedAt": "2026-11-01T12:00:00Z"
        }))
        .unwrap();
        assert_eq!(stop.phase, AutoResumePhase::Unknown);
        assert!(!stop.pending());
    }

    #[test]
    fn pending_needs_enabled_and_unresumed() {
        let mut stop = AutoResume {
            entry_id: "a".into(),
            enabled: true,
            phase: AutoResumePhase::Waiting,
            resume_at: None,
            stopped_at: Utc::now(),
            resumed_at: None,
            attempt: 0,
        };
        assert!(stop.pending());
        stop.phase = AutoResumePhase::Resumed;
        assert!(!stop.pending());
        stop.phase = AutoResumePhase::Waiting;
        stop.enabled = false;
        assert!(!stop.pending());
    }
}
