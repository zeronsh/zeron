//! Session freshness on the phone's clock.
//!
//! A `Working` / `AwaitingInput` session counts as live only while its
//! `updatedAt` is younger than [`zeron_proto::view::SESSION_STALE_MS`] (45 s),
//! and the engine heartbeats running sessions every ~15 s. The rows carry the
//! *computer's* timestamps while the staleness check runs on the *phone's*
//! clock, so a phone that runs half a minute ahead of the computer judged every
//! running session stale and the home list showed no "Working" at all.
//!
//! Presence already avoids this (the direct link stamps the engine device with
//! the phone's receipt time); this does the same for session rows. Whenever a
//! row's `updatedAt` moves, the engine has just touched it, so it is re-stamped
//! with the receipt time on the phone's clock (and `startedAt` shifted by the
//! same amount, so the working timer stays right). A row that has not moved
//! keeps its stamp, so a computer that stops heartbeating still goes stale 45 s
//! later. A row seen for the first time keeps the engine's stamp shifted by the
//! last measured offset, if any.

use std::collections::HashMap;

use chrono::{DateTime, TimeDelta, Utc};
use zeron_proto::Session;

#[derive(Default)]
pub(crate) struct SessionClock {
    /// chat id → (engine `updatedAt`, phone-clock `updatedAt`).
    seen: HashMap<String, (DateTime<Utc>, DateTime<Utc>)>,
    /// Phone minus computer, from the last row that moved (receipt latency
    /// included, so it errs toward "fresher").
    offset: Option<TimeDelta>,
}

impl SessionClock {
    pub(crate) fn offset_ms(&self) -> Option<i64> {
        self.offset.map(|o| o.num_milliseconds())
    }

    pub(crate) fn rebase(&mut self, rows: Vec<Session>, now: DateTime<Utc>) -> Vec<Session> {
        let mut next = HashMap::with_capacity(rows.len());
        let mut out = Vec::with_capacity(rows.len());
        for mut row in rows {
            let raw = row.updated_at;
            let local = match self.seen.get(&row.chat_id) {
                Some((prev_raw, prev_local)) if *prev_raw == raw => *prev_local,
                Some(_) => {
                    self.offset = Some(now - raw);
                    now
                }
                None => match self.offset {
                    Some(offset) => (raw + offset).min(now),
                    None => raw,
                },
            };
            // Millisecond precision, like the workspace doc stores it, so an
            // unchanged row compares equal to its stored copy.
            let local = DateTime::from_timestamp_millis(local.timestamp_millis()).unwrap_or(local);
            let shift = local - raw;
            row.started_at = row.started_at.map(|t| t + shift);
            row.updated_at = local;
            next.insert(row.chat_id.clone(), (raw, local));
            out.push(row);
        }
        self.seen = next;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::SessionStatus;

    fn row(chat: &str, status: SessionStatus, updated: DateTime<Utc>) -> Session {
        Session {
            last_completed_turn: None,
            chat_id: chat.into(),
            device_id: "pc".into(),
            status,
            started_at: Some(updated - TimeDelta::minutes(5)),
            updated_at: updated,
        }
    }

    #[test]
    fn a_heartbeat_is_fresh_on_the_phone_clock_even_when_the_computer_lags() {
        let mut clock = SessionClock::default();
        let phone = DateTime::from_timestamp_millis(Utc::now().timestamp_millis()).unwrap();
        let pc = phone - TimeDelta::seconds(120); // phone runs 2 min ahead
        let first = clock.rebase(vec![row("a", SessionStatus::Working, pc)], phone);
        assert_eq!(
            first[0].updated_at, pc,
            "first sight, no offset yet: engine stamp"
        );
        // Next heartbeat 15 s later: stamped with the receipt time.
        let beat = clock.rebase(
            vec![row(
                "a",
                SessionStatus::Working,
                pc + TimeDelta::seconds(15),
            )],
            phone + TimeDelta::seconds(15),
        );
        assert_eq!(beat[0].updated_at, phone + TimeDelta::seconds(15));
        assert_eq!(
            beat[0].started_at,
            Some(phone + TimeDelta::seconds(15) - TimeDelta::minutes(5)),
            "startedAt moves with it"
        );
        // The same frame again (another row changed): the stamp holds, so a
        // silent computer still goes stale.
        let again = clock.rebase(
            vec![row(
                "a",
                SessionStatus::Working,
                pc + TimeDelta::seconds(15),
            )],
            phone + TimeDelta::seconds(50),
        );
        assert_eq!(again[0].updated_at, phone + TimeDelta::seconds(15));
    }

    #[test]
    fn new_rows_use_the_measured_offset() {
        let mut clock = SessionClock::default();
        let phone = DateTime::from_timestamp_millis(Utc::now().timestamp_millis()).unwrap();
        let pc = phone - TimeDelta::seconds(90);
        clock.rebase(vec![row("a", SessionStatus::Working, pc)], phone);
        clock.rebase(
            vec![row(
                "a",
                SessionStatus::Working,
                pc + TimeDelta::seconds(10),
            )],
            phone + TimeDelta::seconds(10),
        );
        let rows = clock.rebase(
            vec![
                row("a", SessionStatus::Working, pc + TimeDelta::seconds(10)),
                row("b", SessionStatus::Working, pc + TimeDelta::seconds(8)),
            ],
            phone + TimeDelta::seconds(11),
        );
        assert_eq!(rows[1].updated_at, phone + TimeDelta::seconds(8));
    }

    #[test]
    fn rows_that_leave_are_forgotten() {
        let mut clock = SessionClock::default();
        let now = Utc::now();
        clock.rebase(vec![row("a", SessionStatus::Idle, now)], now);
        clock.rebase(vec![], now);
        assert!(clock.seen.is_empty());
    }
}
