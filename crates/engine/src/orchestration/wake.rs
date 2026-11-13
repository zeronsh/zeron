//! Scheduled wakes: an orchestrator hosted on this device with a
//! [`WakeSchedule`] is sent its wake prompt as a run at the scheduled times
//! (every N minutes, or at local "HH:MM" times, weekdays only if asked).
//! Skipped while it's paused, archived or already working; `lastWokeAt`
//! records each due time either way, so one is never sent twice.

use std::time::Duration;

use chrono::{DateTime, Datelike, Local, NaiveTime, TimeZone, Utc, Weekday};
use zeron_proto::SessionStatus;
use zeron_proto::orchestration::{OrchestratorKind, WakeSchedule};

use super::Orchestration;
use super::ops::{When, chat_status, deliver};

/// How often the timer looks.
const TICK: Duration = Duration::from_secs(30);
/// The shortest interval a schedule may have.
pub const MIN_EVERY_MINUTES: u32 = 5;
/// A time of day missed by more than this (the device was off) is skipped.
const LATE_BY_AT_MOST: chrono::Duration = chrono::Duration::minutes(30);
/// What a wake says when its schedule has no prompt.
const DEFAULT_PROMPT: &str =
    "Scheduled check-in: look over your board and crew, and move what's stuck.";

/// `wake` as stored: an interval no shorter than [`MIN_EVERY_MINUTES`], only
/// well-formed times.
pub(crate) fn normalized(mut wake: WakeSchedule) -> WakeSchedule {
    wake.every_minutes = wake.every_minutes.map(|m| m.max(MIN_EVERY_MINUTES));
    wake.at
        .retain(|t| NaiveTime::parse_from_str(t, "%H:%M").is_ok());
    wake.at.sort();
    wake.at.dedup();
    wake
}

/// Whether `wake` is due at `now` (local time zone `tz`).
pub(crate) fn due<Tz: TimeZone>(wake: &WakeSchedule, now: DateTime<Utc>, tz: &Tz) -> bool {
    let local = now.with_timezone(tz);
    if wake.weekdays_only && matches!(local.weekday(), Weekday::Sat | Weekday::Sun) {
        return false;
    }
    let last = wake.last_woke_at;
    if let Some(every) = wake.every_minutes.map(|m| m.max(MIN_EVERY_MINUTES)) {
        let every = chrono::Duration::minutes(i64::from(every));
        if last.is_none_or(|last| now - last >= every) {
            return true;
        }
    }
    wake.at.iter().any(|at| {
        let Ok(time) = NaiveTime::parse_from_str(at, "%H:%M") else {
            return false;
        };
        let Some(today) = local
            .date_naive()
            .and_time(time)
            .and_local_timezone(tz.clone())
            .earliest()
        else {
            return false;
        };
        let today = today.with_timezone(&Utc);
        today <= now && now - today <= LATE_BY_AT_MOST && last.is_none_or(|last| last < today)
    })
}

pub(crate) fn start(orchestration: Orchestration) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let Some(rpc) = orchestration.rpc() else {
                // (The engine's services are gone, or not built yet.)
                if std::sync::Arc::strong_count(&orchestration.inner) <= 1 {
                    break;
                }
                continue;
            };
            wake_due(&orchestration, &rpc, Utc::now());
        }
    });
}

/// Send every due wake on this device. The orchestrators woken.
pub(crate) fn wake_due(
    orchestration: &Orchestration,
    rpc: &crate::rpc::EngineRpc,
    now: DateTime<Utc>,
) -> Vec<String> {
    let device = orchestration.device_id().to_owned();
    let chats = orchestration.workspace().read_chats().unwrap_or_default();
    let mut woken = Vec::new();
    for o in orchestration.state().orchestrators {
        let Some(wake) = o.wake.clone() else { continue };
        if o.paused || o.archived || o.kind != OrchestratorKind::Standard {
            continue;
        }
        if !chats
            .iter()
            .any(|c| c.id == o.id && c.device_id == device && !c.archived)
        {
            continue;
        }
        if !due(&wake, now, &Local) {
            continue;
        }
        orchestration.workspace().write_orchestration(|doc| {
            if let Some(mut row) = doc.orchestrator(&o.id)
                && let Some(wake) = row.wake.as_mut()
            {
                wake.last_woke_at = Some(now);
                doc.upsert_orchestrator(&row);
            }
        });
        if chat_status(rpc, &o.id) != SessionStatus::Idle {
            continue;
        }
        let prompt = if wake.prompt.trim().is_empty() {
            DEFAULT_PROMPT
        } else {
            wake.prompt.as_str()
        };
        match deliver(rpc, &o.id, prompt, None, When::Next) {
            Ok(_) => woken.push(o.id.clone()),
            Err(error) => tracing::warn!(orchestrator = %o.id, %error, "scheduled wake not sent"),
        }
    }
    woken
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn schedule() -> WakeSchedule {
        WakeSchedule {
            every_minutes: None,
            at: vec![],
            weekdays_only: false,
            prompt: String::new(),
            last_woke_at: None,
        }
    }

    #[test]
    fn every_n_minutes_counts_from_the_last_wake_and_never_below_five() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let mut wake = schedule();
        wake.every_minutes = Some(1);
        let now = at("2026-11-11T10:00:00Z");
        assert!(due(&wake, now, &utc), "never woke");
        wake.last_woke_at = Some(at("2026-11-11T09:57:00Z"));
        assert!(!due(&wake, now, &utc), "1 min reads as 5");
        wake.last_woke_at = Some(at("2026-11-11T09:55:00Z"));
        assert!(due(&wake, now, &utc));
        assert_eq!(normalized(wake).every_minutes, Some(5));
    }

    #[test]
    fn a_time_of_day_wakes_once_in_local_time_and_not_hours_late() {
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        let mut wake = schedule();
        wake.at = vec!["09:00".into(), "bad".into()];
        // 09:05 local = 07:05 UTC.
        assert!(due(&wake, at("2026-11-11T07:05:00Z"), &tz));
        assert!(!due(&wake, at("2026-11-11T06:55:00Z"), &tz), "not yet");
        wake.last_woke_at = Some(at("2026-11-11T07:05:00Z"));
        assert!(!due(&wake, at("2026-11-11T07:10:00Z"), &tz), "once");
        wake.last_woke_at = None;
        assert!(
            !due(&wake, at("2026-11-11T11:00:00Z"), &tz),
            "missed by hours"
        );
        assert_eq!(normalized(wake).at, vec!["09:00"]);
    }

    #[test]
    fn weekdays_only_skips_the_weekend() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let mut wake = schedule();
        wake.every_minutes = Some(30);
        wake.weekdays_only = true;
        // 2026-11-14 is a Saturday.
        assert!(!due(&wake, at("2026-11-14T10:00:00Z"), &utc));
        assert!(due(&wake, at("2026-11-13T10:00:00Z"), &utc));
    }
}
