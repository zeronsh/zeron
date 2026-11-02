//! Auto-resume after a usage limit resets: what a limit stop's error card and
//! the session list say about a chat its host will pick back up
//! ([`zeron_proto::AutoResume`], read off the chat's session row).
//!
//! The phone's card says the same (crates/mobile/src/layout/auto_resume.rs
//! keeps a copy of this wording; change both together).

use chrono::{DateTime, Datelike, TimeZone, Utc};
use zeron_proto::{AutoResume, AutoResumePhase};

/// The card's switch: whether it reads on, and whether flipping it can
/// still change anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switch {
    pub on: bool,
    pub interactive: bool,
}

/// The card's switch for `stop`; `None` when it has none.
pub fn switch(stop: &AutoResume) -> Option<Switch> {
    match stop.phase {
        AutoResumePhase::Waiting => Some(Switch {
            on: stop.enabled,
            interactive: true,
        }),
        // Under way: nothing left to switch off.
        AutoResumePhase::Resuming => Some(Switch {
            on: stop.enabled,
            interactive: false,
        }),
        // The host gave up: the switch reads off, and turning it on starts
        // over.
        AutoResumePhase::GaveUp => Some(Switch {
            on: false,
            interactive: true,
        }),
        AutoResumePhase::Resumed | AutoResumePhase::Unknown => None,
    }
}

/// The card's status line, in the viewer's local time. `host_online`: the
/// chat's host is reachable (a waiting stop resumes only once it is).
pub fn status(stop: &AutoResume, host_online: bool, now: DateTime<Utc>) -> String {
    status_in(stop, host_online, now, &chrono::Local)
}

pub fn status_in<Tz: TimeZone>(
    stop: &AutoResume,
    host_online: bool,
    now: DateTime<Utc>,
    tz: &Tz,
) -> String
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
            Some(at) => format!(
                "Auto-resume off · limit resets {}",
                clock_label(at, now, tz)
            ),
            None => "Auto-resume off".into(),
        },
        AutoResumePhase::Waiting if !host_online => {
            "Host offline — auto-resumes once it's back".into()
        }
        AutoResumePhase::Waiting => match (upcoming, stop.resume_at) {
            (Some(at), _) => format!("Auto-resumes {}", clock_label(at, now, tz)),
            // Its time has come: the host is checking the limit is back.
            (None, Some(_)) => "Checking limit…".into(),
            (None, None) => "Auto-resumes when the limit resets".into(),
        },
    }
}

/// When the card's words change by themselves: a waiting stop's reset time.
pub fn next_change(stop: &AutoResume, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    (stop.phase == AutoResumePhase::Waiting)
        .then_some(stop.resume_at)
        .flatten()
        .filter(|at| *at > now)
}

/// By the viewer's calendar: "at 3:40 PM" today, "tomorrow at 3:40 PM",
/// "yesterday at 3:40 PM", "Thu at 3:40 PM" later this week, else
/// "on Nov 14 at 3:40 PM".
pub fn clock_label<Tz: TimeZone>(at: DateTime<Utc>, now: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let local = at.with_timezone(tz);
    let days = local.date_naive().num_days_from_ce()
        - now.with_timezone(tz).date_naive().num_days_from_ce();
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
pub(crate) fn stop(enabled: bool, phase: AutoResumePhase) -> AutoResume {
    AutoResume {
        entry_id: "a1".into(),
        enabled,
        phase,
        resume_at: None,
        stopped_at: Utc.with_ymd_and_hms(2026, 11, 2, 13, 0, 0).unwrap(),
        resumed_at: None,
        attempt: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, FixedOffset};

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 2, h, m, 0).unwrap()
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    #[test]
    fn status_lines_name_the_local_reset_time() {
        let tz = utc();
        let now = at(14, 0);
        let mut waiting = stop(true, AutoResumePhase::Waiting);
        assert_eq!(
            status_in(&waiting, true, now, &tz),
            "Auto-resumes when the limit resets"
        );
        waiting.resume_at = Some(at(15, 40));
        assert_eq!(
            status_in(&waiting, true, now, &tz),
            "Auto-resumes at 3:40 PM"
        );
        assert_eq!(next_change(&waiting, now), Some(at(15, 40)));
        assert_eq!(
            switch(&waiting),
            Some(Switch {
                on: true,
                interactive: true
            })
        );
        assert_eq!(
            status_in(&waiting, false, now, &tz),
            "Host offline — auto-resumes once it's back"
        );
        // Past its time, the host is confirming the limit is back.
        waiting.resume_at = Some(at(13, 59));
        assert_eq!(status_in(&waiting, true, now, &tz), "Checking limit…");
        assert_eq!(next_change(&waiting, now), None);

        // Resuming reads as such even with the stop's switch off.
        let mut resuming = stop(false, AutoResumePhase::Resuming);
        assert_eq!(status_in(&resuming, true, now, &tz), "Resuming…");
        resuming.enabled = true;
        assert_eq!(switch(&resuming).map(|s| s.interactive), Some(false));

        let mut resumed = stop(true, AutoResumePhase::Resumed);
        resumed.resumed_at = Some(at(15, 41));
        assert_eq!(status_in(&resumed, true, now, &tz), "Resumed at 3:41 PM");
        assert_eq!(switch(&resumed), None);

        let mut off = stop(false, AutoResumePhase::Waiting);
        assert_eq!(status_in(&off, true, now, &tz), "Auto-resume off");
        off.resume_at = Some(at(15, 40));
        assert_eq!(
            status_in(&off, true, now, &tz),
            "Auto-resume off · limit resets at 3:40 PM"
        );
        assert_eq!(switch(&off).map(|s| s.on), Some(false));

        let gave_up = stop(true, AutoResumePhase::GaveUp);
        assert_eq!(
            status_in(&gave_up, true, now, &tz),
            "Auto-resume stopped — send a message to continue"
        );
        assert_eq!(
            switch(&gave_up),
            Some(Switch {
                on: false,
                interactive: true
            })
        );
        assert_eq!(switch(&stop(true, AutoResumePhase::Unknown)), None);
    }

    #[test]
    fn labels_follow_the_calendar() {
        let tz = utc();
        let late = Utc.with_ymd_and_hms(2026, 11, 2, 23, 50, 0).unwrap();
        // Across midnight: ten minutes away, but tomorrow.
        assert_eq!(
            clock_label(late + Duration::minutes(20), late, &tz),
            "tomorrow at 12:10 AM"
        );
        // 21 hours away: today from the small hours, tomorrow from the evening.
        let morning = at(1, 0);
        assert_eq!(
            clock_label(morning + Duration::hours(21), morning, &tz),
            "at 10:00 PM"
        );
        let evening = at(20, 0);
        assert_eq!(
            clock_label(evening + Duration::hours(21), evening, &tz),
            "tomorrow at 5:00 PM"
        );
        assert_eq!(
            clock_label(evening - Duration::hours(21), evening, &tz),
            "yesterday at 11:00 PM"
        );
        let now = at(14, 0);
        assert_eq!(
            clock_label(now + Duration::days(3), now, &tz),
            "Thu at 2:00 PM"
        );
        // 6.9 days: a weekday while it is six calendar days out, a date once
        // it is seven.
        let soon = Duration::minutes(6 * 24 * 60 + 21 * 60 + 36);
        assert_eq!(clock_label(morning + soon, morning, &tz), "Sun at 10:36 PM");
        assert_eq!(clock_label(now + soon, now, &tz), "on Nov 9 at 11:36 AM");
        // Another zone moves the calendar day.
        let tokyo = FixedOffset::east_opt(9 * 3600).unwrap();
        assert_eq!(clock_label(at(16, 0), now, &tokyo), "tomorrow at 1:00 AM");
    }
}
