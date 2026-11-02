//! The inbox sidebar's rules: which section a session sits in, how each
//! section orders, and the snooze presets. Pure, so the layout, the row
//! actions and the jump shortcuts all agree.

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone, Utc, Weekday};
use zeron_proto::{Chat, ChatIndicator};

/// A session's place in the inbox sidebar, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum InboxSection {
    Pinned,
    Inbox,
    Working,
    Snoozed,
    Settled,
}

/// Where `chat` sits. `status` is the row's display status
/// (`AppState::display_status_for`, so a send in flight reads Working);
/// `working_shelf` is the "Working section" setting. First match wins.
pub(crate) fn inbox_section(
    chat: &Chat,
    status: ChatIndicator,
    pinned: bool,
    working_shelf: bool,
    now: DateTime<Utc>,
) -> InboxSection {
    if chat.archived {
        InboxSection::Settled
    } else if pinned {
        InboxSection::Pinned
    } else if chat.snoozed_at(now) && !raises_hand(status) {
        InboxSection::Snoozed
    } else if working_shelf && status == ChatIndicator::Working {
        InboxSection::Working
    } else {
        InboxSection::Inbox
    }
}

/// A snoozed chat wakes early when its agent needs the user or fails; a
/// normal finished run does not wake it.
pub(crate) fn raises_hand(status: ChatIndicator) -> bool {
    matches!(status, ChatIndicator::AwaitingInput | ChatIndicator::Errored)
}

/// Inbox/Working order key, newest first: last activity, or the wake time
/// of a snooze that has passed, so a chat that just woke lands on top.
pub(crate) fn inbox_recency(chat: &Chat, now: DateTime<Utc>) -> DateTime<Utc> {
    let activity = chat.last_message_at.unwrap_or(chat.created_at);
    match chat.snoozed_until {
        Some(woke) if woke <= now => activity.max(woke),
        _ => activity,
    }
}

/// Settled order key, newest first: when it was settled, else its activity.
pub(crate) fn settled_recency(chat: &Chat) -> DateTime<Utc> {
    chat.archived_at
        .unwrap_or_else(|| chat.last_message_at.unwrap_or(chat.created_at))
}

/// Whether the Snooze menu is offered for a row in `section`: live rows
/// only, and not while the agent is waiting on the user.
pub(crate) fn can_snooze(section: InboxSection, status: ChatIndicator) -> bool {
    matches!(section, InboxSection::Inbox | InboxSection::Working)
        && status != ChatIndicator::AwaitingInput
}

/// The Snooze submenu's choices, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnoozePreset {
    OneHour,
    ThreeHours,
    ThisEvening,
    Tomorrow,
    NextWeek,
}

impl SnoozePreset {
    pub(crate) const ALL: [SnoozePreset; 5] = [
        SnoozePreset::OneHour,
        SnoozePreset::ThreeHours,
        SnoozePreset::ThisEvening,
        SnoozePreset::Tomorrow,
        SnoozePreset::NextWeek,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            SnoozePreset::OneHour => "In 1 hour",
            SnoozePreset::ThreeHours => "In 3 hours",
            SnoozePreset::ThisEvening => "This evening",
            SnoozePreset::Tomorrow => "Tomorrow",
            SnoozePreset::NextWeek => "Next week",
        }
    }

    /// When a snooze chosen at `now` wakes, in `tz`; `None` when the preset
    /// is not offered right now ("This evening" from 17:00 on).
    pub(crate) fn wake_at<Tz: TimeZone>(self, now: DateTime<Tz>) -> Option<DateTime<Utc>> {
        let at = |date: chrono::NaiveDate, hour: u32| {
            let time = NaiveTime::from_hms_opt(hour, 0, 0)?;
            now.timezone()
                .from_local_datetime(&date.and_time(time))
                .earliest()
                .map(|wake| wake.with_timezone(&Utc))
        };
        let today = now.date_naive();
        match self {
            SnoozePreset::OneHour => Some((now + Duration::hours(1)).with_timezone(&Utc)),
            SnoozePreset::ThreeHours => Some((now + Duration::hours(3)).with_timezone(&Utc)),
            SnoozePreset::ThisEvening => {
                (now.time() < NaiveTime::from_hms_opt(17, 0, 0)?).then(|| at(today, 18))?
            }
            SnoozePreset::Tomorrow => at(today.succ_opt()?, 9),
            SnoozePreset::NextWeek => {
                let ahead = 7 - today.weekday().num_days_from_monday();
                let monday = today + Duration::days(i64::from(ahead));
                debug_assert_eq!(monday.weekday(), Weekday::Mon);
                at(monday, 9)
            }
        }
    }

    /// [`Self::wake_at`] in the machine's local time zone.
    pub(crate) fn wake_at_local(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.wake_at(now.with_timezone(&Local))
    }
}

/// A snoozed row's corner: time until it wakes ("45m", "3h", "2d").
pub(crate) fn format_time_until(until: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let minutes = (until - now).num_minutes().max(1);
    if minutes < 60 {
        format!("{minutes}m")
    } else if minutes < 60 * 24 {
        format!("{}h", minutes / 60)
    } else {
        format!("{}d", minutes / (60 * 24))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn chat(minutes_ago: i64, now: DateTime<Utc>) -> Chat {
        let at = now - Duration::minutes(minutes_ago);
        serde_json::from_value(serde_json::json!({
            "id": "c1",
            "deviceId": "d1",
            "title": null,
            "archived": false,
            "cwd": null,
            "branch": null,
            "checkoutId": null,
            "config": null,
            "lastMessagePreview": null,
            "lastMessageAt": at,
            "createdAt": at,
        }))
        .unwrap()
    }

    #[test]
    fn sections_follow_first_match() {
        let now = Utc::now();
        let mut c = chat(5, now);
        let idle = ChatIndicator::Idle;
        assert_eq!(inbox_section(&c, idle, false, true, now), InboxSection::Inbox);
        assert_eq!(
            inbox_section(&c, ChatIndicator::Working, false, true, now),
            InboxSection::Working
        );
        assert_eq!(
            inbox_section(&c, ChatIndicator::Working, false, false, now),
            InboxSection::Inbox
        );
        assert_eq!(inbox_section(&c, idle, true, true, now), InboxSection::Pinned);
        c.snoozed_until = Some(now + Duration::hours(1));
        assert_eq!(inbox_section(&c, idle, false, true, now), InboxSection::Snoozed);
        // Early wake: the agent needs the user.
        assert_eq!(
            inbox_section(&c, ChatIndicator::AwaitingInput, false, true, now),
            InboxSection::Inbox
        );
        // A finished run does not wake it.
        assert_eq!(
            inbox_section(&c, ChatIndicator::Completed, false, true, now),
            InboxSection::Snoozed
        );
        c.archived = true;
        assert_eq!(inbox_section(&c, idle, true, true, now), InboxSection::Settled);
    }

    #[test]
    fn helpers_place_their_parent() {
        use crate::state::{HelperActivity, inbox_status};
        let now = Utc::now();
        let mut c = chat(5, now);
        let idle = ChatIndicator::Idle;
        let working = HelperActivity {
            working: true,
            ..Default::default()
        };
        let asking = HelperActivity {
            awaiting_input: true,
            ..Default::default()
        };
        // Subagents or a working side chat keep an idle parent Working…
        for status in [inbox_status(idle, 1, Default::default()), inbox_status(idle, 0, working)] {
            assert_eq!(inbox_section(&c, status, false, true, now), InboxSection::Working);
            // …or in the Inbox with the Working section off.
            assert_eq!(inbox_section(&c, status, false, false, now), InboxSection::Inbox);
        }
        // A side chat asking the user pulls its parent into the Inbox, and
        // wakes it from a snooze.
        let status = inbox_status(ChatIndicator::Working, 2, asking);
        assert_eq!(inbox_section(&c, status, false, true, now), InboxSection::Inbox);
        c.snoozed_until = Some(now + Duration::hours(1));
        assert_eq!(inbox_section(&c, status, false, true, now), InboxSection::Inbox);
        assert!(!can_snooze(InboxSection::Inbox, status));
        // Working helpers alone don't wake it.
        let busy = inbox_status(idle, 1, working);
        assert_eq!(inbox_section(&c, busy, false, true, now), InboxSection::Snoozed);
    }

    #[test]
    fn a_woken_chat_sorts_by_its_wake_time() {
        let now = Utc::now();
        let mut c = chat(120, now);
        let woke = now - Duration::minutes(1);
        c.snoozed_until = Some(woke);
        assert_eq!(inbox_recency(&c, now), woke);
        c.snoozed_until = Some(now + Duration::minutes(1));
        assert_eq!(inbox_recency(&c, now), now - Duration::minutes(120));
    }

    #[test]
    fn presets_land_on_local_wall_clock_times() {
        let tz = FixedOffset::east_opt(7 * 3600).unwrap();
        // Wednesday 2026-10-28 10:30 local.
        let now = tz.with_ymd_and_hms(2026, 10, 28, 10, 30, 0).unwrap();
        let local = |wake: Option<DateTime<Utc>>| wake.unwrap().with_timezone(&tz);
        assert_eq!(
            local(SnoozePreset::ThisEvening.wake_at(now)),
            tz.with_ymd_and_hms(2026, 10, 28, 18, 0, 0).unwrap()
        );
        assert_eq!(
            local(SnoozePreset::Tomorrow.wake_at(now)),
            tz.with_ymd_and_hms(2026, 10, 29, 9, 0, 0).unwrap()
        );
        assert_eq!(
            local(SnoozePreset::NextWeek.wake_at(now)),
            tz.with_ymd_and_hms(2026, 11, 2, 9, 0, 0).unwrap()
        );
        let late = tz.with_ymd_and_hms(2026, 10, 28, 17, 30, 0).unwrap();
        assert_eq!(SnoozePreset::ThisEvening.wake_at(late), None);
        // From a Monday, next week is the following Monday.
        let monday = tz.with_ymd_and_hms(2026, 11, 2, 8, 0, 0).unwrap();
        assert_eq!(
            local(SnoozePreset::NextWeek.wake_at(monday)),
            tz.with_ymd_and_hms(2026, 11, 9, 9, 0, 0).unwrap()
        );
    }

    #[test]
    fn time_until_rounds_down_to_the_unit() {
        let now = Utc::now();
        assert_eq!(format_time_until(now + Duration::minutes(45), now), "45m");
        assert_eq!(format_time_until(now + Duration::minutes(200), now), "3h");
        assert_eq!(format_time_until(now + Duration::hours(50), now), "2d");
    }
}
