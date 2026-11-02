//! A usage limit stop's auto-resume on the phone's error card: which card
//! shows it and what its status line says. The words are the desktop card's
//! (crates/ui/src/auto_resume.rs keeps the same; change both together).

use std::sync::Arc;

use chrono::{DateTime, Datelike, TimeZone, Utc};
use zeron_doc::{MessagePart, SessionMessageEntry};
use zeron_proto::{AutoResume, AutoResumePhase};

use super::rows::ResumeInput;

/// The stopped entry's last error card, with its status line in the
/// viewer's local time, and its switch while there is anything to switch.
/// `host_online`: the chat's host is reachable.
pub(crate) fn resume_inputs(stop: Option<&AutoResume>, host_online: bool, entries: &[Arc<SessionMessageEntry>], now: DateTime<Utc>) -> Vec<ResumeInput> {
    resume_inputs_in(stop, host_online, entries, now, &chrono::Local)
}

pub(crate) fn resume_inputs_in<Tz: TimeZone>(
    stop: Option<&AutoResume>,
    host_online: bool,
    entries: &[Arc<SessionMessageEntry>],
    now: DateTime<Utc>,
    tz: &Tz,
) -> Vec<ResumeInput>
where
    Tz::Offset: std::fmt::Display,
{
    let Some(stop) = stop else {
        return Vec::new();
    };
    let Some(entry) = entries.iter().rev().find(|e| e.id == stop.entry_id) else {
        return Vec::new();
    };
    let Some(part) = entry.parts.iter().rev().find_map(|p| match p {
        MessagePart::Error { id, .. } => Some(id),
        _ => None,
    }) else {
        return Vec::new();
    };
    vec![ResumeInput {
        part: format!("{}#{}", entry.id, part),
        status: status_in(stop, host_online, now, tz),
        switch: switch(stop),
    }]
}

/// The card's switch, reading on or off: a waiting stop's, and a stop the
/// host gave up on (off; turning it on starts over). None otherwise.
fn switch(stop: &AutoResume) -> Option<bool> {
    match stop.phase {
        AutoResumePhase::Waiting => Some(stop.enabled),
        AutoResumePhase::GaveUp => Some(false),
        AutoResumePhase::Resuming | AutoResumePhase::Resumed | AutoResumePhase::Unknown => None,
    }
}

/// When the card's words change by themselves: a waiting stop's reset time.
pub(crate) fn next_change(stop: &AutoResume, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    (stop.phase == AutoResumePhase::Waiting).then_some(stop.resume_at).flatten().filter(|at| *at > now)
}

fn status_in<Tz: TimeZone>(stop: &AutoResume, host_online: bool, now: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let upcoming = stop.resume_at.filter(|at| *at > now);
    match stop.phase {
        AutoResumePhase::Resumed => match stop.resumed_at {
            Some(at) => format!("Resumed {}", clock_label(at, now, tz)),
            None => "Resumed".into(),
        },
        AutoResumePhase::GaveUp => "Auto-resume stopped — send a message to continue".into(),
        AutoResumePhase::Unknown => "Update Zeron to see this chat's auto-resume".into(),
        AutoResumePhase::Resuming => "Resuming…".into(),
        AutoResumePhase::Waiting if !stop.enabled => match upcoming {
            Some(at) => format!("Auto-resume off · limit resets {}", clock_label(at, now, tz)),
            None => "Auto-resume off".into(),
        },
        AutoResumePhase::Waiting if !host_online => "Host offline — auto-resumes once it's back".into(),
        AutoResumePhase::Waiting => match (upcoming, stop.resume_at) {
            (Some(at), _) => format!("Auto-resumes {}", clock_label(at, now, tz)),
            (None, Some(_)) => "Checking limit…".into(),
            (None, None) => "Auto-resumes when the limit resets".into(),
        },
    }
}

/// By the viewer's calendar: "at 3:40 PM" today, "tomorrow at 3:40 PM",
/// "yesterday at 3:40 PM", "Thu at 3:40 PM" later this week, else
/// "on Nov 14 at 3:40 PM".
fn clock_label<Tz: TimeZone>(at: DateTime<Utc>, now: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let local = at.with_timezone(tz);
    let days = local.date_naive().num_days_from_ce() - now.with_timezone(tz).date_naive().num_days_from_ce();
    let time = local.format("%-I:%M %p");
    match days {
        0 => format!("at {time}"),
        1 => format!("tomorrow at {time}"),
        -1 => format!("yesterday at {time}"),
        2..=6 => format!("{} at {time}", local.format("%a")),
        _ => format!("on {} at {time}", local.format("%b %-d")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, FixedOffset};

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 2, h, m, 0).unwrap()
    }

    fn stop(enabled: bool, phase: AutoResumePhase) -> AutoResume {
        AutoResume { entry_id: "a1".into(), enabled, phase, resume_at: None, stopped_at: at(13, 0), resumed_at: None, attempt: 0 }
    }

    #[test]
    fn status_lines_match_the_desktop_card() {
        let tz = FixedOffset::east_opt(0).unwrap();
        let now = at(14, 0);
        let mut waiting = stop(true, AutoResumePhase::Waiting);
        assert_eq!(status_in(&waiting, true, now, &tz), "Auto-resumes when the limit resets");
        waiting.resume_at = Some(at(15, 40));
        assert_eq!(status_in(&waiting, true, now, &tz), "Auto-resumes at 3:40 PM");
        assert_eq!(status_in(&waiting, false, now, &tz), "Host offline — auto-resumes once it's back");
        assert_eq!(next_change(&waiting, now), Some(at(15, 40)));
        assert_eq!(switch(&waiting), Some(true));
        waiting.resume_at = Some(at(13, 59));
        assert_eq!(status_in(&waiting, true, now, &tz), "Checking limit…");
        assert_eq!(status_in(&stop(false, AutoResumePhase::Resuming), true, now, &tz), "Resuming…");
        assert_eq!(switch(&stop(true, AutoResumePhase::Resuming)), None);
        let mut off = stop(false, AutoResumePhase::Waiting);
        off.resume_at = Some(at(15, 40));
        assert_eq!(status_in(&off, true, now, &tz), "Auto-resume off · limit resets at 3:40 PM");
        let gave_up = stop(true, AutoResumePhase::GaveUp);
        assert_eq!(status_in(&gave_up, true, now, &tz), "Auto-resume stopped — send a message to continue");
        assert_eq!(switch(&gave_up), Some(false));
        assert_eq!(switch(&stop(true, AutoResumePhase::Unknown)), None);
    }

    #[test]
    fn labels_follow_the_calendar() {
        let tz = FixedOffset::east_opt(0).unwrap();
        let late = Utc.with_ymd_and_hms(2026, 11, 2, 23, 50, 0).unwrap();
        assert_eq!(clock_label(late + Duration::minutes(20), late, &tz), "tomorrow at 12:10 AM");
        let morning = at(1, 0);
        assert_eq!(clock_label(morning + Duration::hours(21), morning, &tz), "at 10:00 PM");
        let evening = at(20, 0);
        assert_eq!(clock_label(evening + Duration::hours(21), evening, &tz), "tomorrow at 5:00 PM");
        assert_eq!(clock_label(evening - Duration::hours(21), evening, &tz), "yesterday at 11:00 PM");
        let now = at(14, 0);
        assert_eq!(clock_label(now + Duration::days(3), now, &tz), "Thu at 2:00 PM");
        let soon = Duration::minutes(6 * 24 * 60 + 21 * 60 + 36);
        assert_eq!(clock_label(morning + soon, morning, &tz), "Sun at 10:36 PM");
        assert_eq!(clock_label(now + soon, now, &tz), "on Nov 9 at 11:36 AM");
    }
}
