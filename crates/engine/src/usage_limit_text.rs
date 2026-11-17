//! Reading a provider's usage-limit stop from its words: whether an error
//! is the account's own usage limit (one that resets by itself), which
//! window, and when it resets. The harnesses that can say so structurally
//! do ([`zeron_proto::AgentEvent::LimitReached`]); this reads the rest, and
//! the reset time a structured signal left out.

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone, Utc};

/// How many resumes in a row may run straight back into the limit before
/// the host stops resuming the chat by itself.
pub(crate) const MAX_ATTEMPTS: u32 = 8;

/// The least a resume that ran straight back into the limit waits before
/// the next one, growing with each attempt in a row.
pub(crate) fn check_backoff(attempt: u32) -> Duration {
    const BACKOFF: [Duration; 4] = [
        Duration::minutes(5),
        Duration::minutes(15),
        Duration::minutes(30),
        Duration::minutes(60),
    ];
    BACKOFF[(attempt as usize).min(BACKOFF.len() - 1)]
}

/// A turn the usage limit stopped, as read at its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LimitHit {
    /// "5-hour", "weekly", "usage", "quota".
    pub window: Option<String>,
    /// When it resets, when the provider said (always after the turn's end).
    pub until: Option<DateTime<Utc>>,
}

/// Whether a turn that failed with `errors` (its error parts and the Done's
/// error) was stopped by the account's usage limit, and when that resets.
/// `signal`: the harness's own word ([`zeron_proto::AgentEvent::LimitReached`]:
/// its window and reset time). Without it, the words decide
/// ([`limit_stop_text`]). No word makes a stop of a limit no reset clears
/// ([`limit_never_resets`]). A reset not after `now` says nothing.
pub(crate) fn limit_hit(
    signal: Option<&(Option<String>, Option<DateTime<Utc>>)>,
    errors: &[&str],
    now: DateTime<Utc>,
) -> Option<LimitHit> {
    if errors.iter().any(|message| limit_never_resets(message, now)) {
        return None;
    }
    let worded = errors.iter().find(|message| limit_stop_text(message));
    if signal.is_none() && worded.is_none() {
        return None;
    }
    let ahead = |at: &DateTime<Utc>| *at > now;
    let until = signal
        .and_then(|(_, at)| *at)
        .filter(ahead)
        .or_else(|| errors.iter().find_map(|message| reset_time_in(message, now)).filter(ahead));
    let window = signal
        .and_then(|(window, _)| window.clone())
        .or_else(|| errors.iter().find_map(|message| window_in(message)))
        .or_else(|| Some("usage".to_owned()));
    Some(LimitHit { window, until })
}

/// Which window `message` names.
fn window_in(message: &str) -> Option<String> {
    let text = normalize(message);
    if !limit_stop_text(&text) {
        return None;
    }
    let window = if text.contains("5-hour") || text.contains("5 hour") || text.contains("five-hour") {
        "5-hour"
    } else if text.contains("weekly") || text.contains("7-day") || text.contains("seven-day") {
        "weekly"
    } else if text.contains("quota") {
        "quota"
    } else {
        "usage"
    };
    Some(window.to_owned())
}

/// Whether `message` reads as a usage limit that resets by itself: the
/// account's own plan window or quota. Takes "usage limit reached" /
/// "you've hit your" wording, Claude's ("Claude 5-hour limit reached — the
/// turn was blocked", "Claude usage limit reached", "Claude rate limit
/// reached") and Codex's ("Codex usage limit reached — it resets in 2h
/// 14m"); none of what reads like a limit but isn't one a reset clears
/// ([`limit_never_resets`]) or is a short throttle (per-minute rates, "try
/// again in 20s"), a cap on turns, tool calls or output, a session limit, a
/// free tier or an upgrade prompt.
pub(crate) fn limit_stop_text(message: &str) -> bool {
    const STOPS: &[&str] = &[
        "you've hit your",
        "you have hit your",
        "usage limit reached",
        "usage limit was reached",
        "usage limit has been reached",
        "usage limit exceeded",
        "hit your limit",
        "hit your usage limit",
        "hour limit reached",
        "weekly limit reached",
        "daily limit reached",
        "limit reached — the turn was blocked",
        "claude rate limit reached",
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
/// spend cap that says no time it resets.
pub(crate) fn limit_never_resets(message: &str, now: DateTime<Utc>) -> bool {
    const SPENT: &[&str] = &["spend cap", "spend limit", "out of credits", "out of usage credits", "usage credit limit"];
    let text = normalize(message);
    never_resets(&text) || (SPENT.iter().any(|spent| text.contains(spent)) && reset_time_in(message, now).is_none())
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
    ["try again in ", "retry in ", "retry after ", "resets in "].iter().any(|marker| {
        text.match_indices(marker)
            .any(|(at, _)| parse_wait(&text[at + marker.len()..]).is_some_and(|wait| wait < Duration::minutes(2)))
    })
}

/// Lowercase, with the typographic marks providers use made plain.
fn normalize(message: &str) -> String {
    message.to_lowercase().replace(['\u{2019}', '\u{2018}'], "'").replace(['\u{a0}', '\u{202f}'], " ")
}

/// When the limit that `message` reports resets, if the text says, as an
/// instant at most a week and a day after `now`:
/// - a wait: Codex's "it resets in 2h 14m", "try again in 20 minutes";
/// - Claude's older "…limit reached|1767225600" (Unix seconds or millis);
/// - a wall time: Claude's "resets 3pm (Europe/Berlin)", "resets Nov 3,
///   10am (UTC)"; Codex's "try again at 3:40 PM". A time is read in the
///   zone it names when that is this device's or UTC, and in this device's
///   zone when it names none; another zone can't be resolved here (`None`).
///   A time without a date is its next occurrence after `now`, except
///   Codex's ("try again at"), which leaves the date out only for today.
pub(crate) fn reset_time_in(message: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let text = normalize(message);
    let found = epoch_in(&text)
        .or_else(|| {
            ["resets in ", "reset in ", "try again in ", "retry in ", "retry after "].iter().find_map(|marker| {
                let at = text.find(marker)?;
                parse_wait(&text[at + marker.len()..]).map(|wait| now + wait)
            })
        })
        .or_else(|| {
            ["resets at ", "resets ", "reset at ", "try again at "].iter().find_map(|marker| {
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
    let digits: String = tail.trim().chars().take_while(char::is_ascii_digit).collect();
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
        let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        if digits == 0 {
            break;
        }
        let Ok(value) = rest[..digits].parse::<i64>() else {
            break;
        };
        let after = rest[digits..].trim_start();
        let unit_len = after.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(after.len());
        let step = match &after[..unit_len] {
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

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

#[derive(Clone, Copy)]
enum Zone {
    Local,
    Utc,
}

/// The digits leading `text`, at most `max` of them, and the rest.
fn number(text: &str, max: usize) -> Option<(u32, &str)> {
    let digits = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
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
        let after = rest.trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == '.').trim_start();
        let (day, after) = number(after, 2)?;
        let after = ["st", "nd", "rd", "th"].iter().find_map(|suffix| after.strip_prefix(suffix)).unwrap_or(after);
        let mut year = None;
        rest = after.trim_start_matches([',', ' ']);
        if let Some((y, after)) = number(rest, 4)
            .filter(|(y, after)| *y >= 2000 && !after.starts_with(':') && !after.trim_start().starts_with(['a', 'p']))
        {
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
        let digits = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
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
    // "(Europe/Berlin)", "(UTC)", not "(in 1h 5m)".
    let zone = match rest
        .trim_start()
        .strip_prefix('(')
        .and_then(|z| z.split_once(')'))
        .filter(|(name, _)| !name.trim().contains(' '))
    {
        None => Zone::Local,
        Some((name, _)) => match name.trim() {
            "utc" | "gmt" | "etc/utc" | "etc/gmt" | "z" => Zone::Utc,
            name if iana_time_zone::get_timezone().is_ok_and(|local| local.eq_ignore_ascii_case(name)) => Zone::Local,
            _ => return None,
        },
    };
    let time = NaiveTime::from_hms_opt(hour, minute, 0)?;
    let at = |year: i32, month: u32, day: u32| {
        let naive = chrono::NaiveDate::from_ymd_opt(year, month, day)?.and_time(time);
        match zone {
            Zone::Utc => Some(Utc.from_utc_datetime(&naive)),
            // Ambiguous across a DST change: the later one is never early.
            Zone::Local => Local.from_local_datetime(&naive).latest().map(|t| t.with_timezone(&Utc)),
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
            if this_year < now - Duration::days(2) { at(today.year() + 1, month, day) } else { Some(this_year) }
        }
        None => {
            let same_day = at(today.year(), today.month(), today.day())?;
            if same_day > now - Duration::minutes(1) {
                Some(same_day)
            } else if today_only {
                Some(now)
            } else {
                let tomorrow = today.succ_opt()?;
                at(tomorrow.year(), tomorrow.month(), tomorrow.day())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339).unwrap().into()
    }

    #[test]
    fn reads_waits_unix_times_and_named_zones() {
        let now = at("2026-11-01T12:00:00Z");
        assert_eq!(
            reset_time_in("Codex usage limit reached — it resets in 2h 14m.", now),
            Some(now + Duration::minutes(134))
        );
        assert_eq!(reset_time_in("Codex usage limit reached — it resets in 3d 2h.", now), Some(now + Duration::hours(74)));
        assert_eq!(
            reset_time_in("Claude AI usage limit reached|1793620800", now),
            DateTime::from_timestamp(1_793_620_800, 0)
        );
        assert_eq!(
            reset_time_in("You've hit your 5-hour limit · resets 3pm (UTC)", now),
            Some(at("2026-11-01T15:00:00Z"))
        );
        assert_eq!(
            reset_time_in("You've hit your weekly limit · resets Nov 3, 10:30am (UTC)", now),
            Some(at("2026-11-03T10:30:00Z"))
        );
        // Past today in UTC: tomorrow's.
        assert_eq!(reset_time_in("You've hit your limit · resets 11am (UTC)", now), Some(at("2026-11-02T11:00:00Z")));
        // A zone this device can't resolve: unknown, not guessed.
        assert_eq!(reset_time_in("You've hit your limit · resets 3pm (Mars/Olympus_Mons)", now), None);
        for none in [
            "Claude 5-hour limit reached — the turn was blocked. Try again after it resets.",
            "429 Too Many Requests",
            "The counter resets 5 times a day",
            "Usage resets in 40d",
        ] {
            assert_eq!(reset_time_in(none, now), None, "{none}");
        }
    }

    #[test]
    fn tells_a_resetting_limit_from_its_look_alikes() {
        for stop in [
            "Claude 5-hour limit reached — the turn was blocked. Try again after it resets.",
            "Claude weekly (Opus) limit reached — the turn was blocked. Try again after it resets.",
            "Claude usage limit reached — try again after the limit resets.",
            "Claude rate limit reached — try again after it resets. API Error: 429 rate_limit_error (resets 3pm)",
            "Codex usage limit reached — it resets in 2h 14m.",
            "Codex usage limit reached.",
            "You've hit your 5-hour limit · resets 3pm (Europe/Berlin)",
            "The agent's usage limit was reached — the turn stopped. Try again after it resets.",
            "Gemini quota will reset after 5h.",
        ] {
            assert!(limit_stop_text(stop), "{stop}");
        }
        for other in [
            "API Error: Server is temporarily limiting requests (not your usage limit)",
            "API Error: Request rejected (429) · rate limited",
            "Quota exceeded. Check your plan and billing details.",
            "Codex usage limit reached. To use Codex with your ChatGPT plan, upgrade to Plus",
            "You hit your spend cap set by your admin.",
            "Claude is overloaded right now — try again shortly.",
            "Rate limit reached for gpt-5 in organization org-x on tokens per min (TPM): Limit 30000.",
            "Usage limit reached — try again in 30 seconds",
            "You've hit your limit of 50 turns for this task",
            "Context limit reached",
            "Harness exited with status 1",
        ] {
            assert!(!limit_stop_text(other), "{other}");
        }
    }

    #[test]
    fn a_hit_takes_the_signal_or_the_words_and_its_time() {
        let now = at("2026-11-01T12:00:00Z");
        let later = now + Duration::hours(2);
        // The harness's own word, with its time.
        let signal = (Some("5-hour".to_owned()), Some(later));
        assert_eq!(
            limit_hit(Some(&signal), &["Claude 5-hour limit reached — the turn was blocked."], now),
            Some(LimitHit { window: Some("5-hour".into()), until: Some(later) })
        );
        // Its word without a time: the text's.
        let bare = (None, None);
        assert_eq!(
            limit_hit(Some(&bare), &["You've hit your weekly limit · resets Nov 3, 10:30am (UTC)"], now),
            Some(LimitHit { window: Some("weekly".into()), until: Some(at("2026-11-03T10:30:00Z")) })
        );
        // Words alone.
        assert_eq!(
            limit_hit(None, &["Codex usage limit reached — it resets in 2h 14m."], now),
            Some(LimitHit { window: Some("usage".into()), until: Some(now + Duration::minutes(134)) })
        );
        assert_eq!(
            limit_hit(None, &["Claude 5-hour limit reached — the turn was blocked. Try again after it resets."], now),
            Some(LimitHit { window: Some("5-hour".into()), until: None })
        );
        assert_eq!(limit_hit(None, &["Harness exited with status 1"], now), None);
        // A reset already past says nothing about when.
        let past = (None, Some(now - Duration::minutes(5)));
        assert_eq!(limit_hit(Some(&past), &["x"], now).unwrap().until, None);
        // No word makes a stop of a limit no reset clears.
        assert_eq!(limit_hit(Some(&signal), &["You've hit your usage limit · contact your admin"], now), None);
    }
}
