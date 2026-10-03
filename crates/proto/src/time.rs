//! Time used for process-local deadlines. Civil time is an initial label,
//! never an elapsed-time source. No timezone or clock synchronization is assumed.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::{Device, Session, SessionStatus};
use chrono::{DateTime, Utc};

pub struct RuntimeClock {
    epoch_ms: i64,
    started: Instant,
}

impl RuntimeClock {
    pub fn new(epoch_ms: i64) -> Self {
        Self {
            epoch_ms,
            started: Instant::now(),
        }
    }

    pub fn at_elapsed(&self, elapsed: Duration) -> i64 {
        self.epoch_ms
            .saturating_add(i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
    }

    pub fn now_ms(&self) -> i64 {
        self.at_elapsed(self.started.elapsed())
    }
}

/// An epoch-shaped process clock advancing only with monotonic elapsed time.
/// Suitable for runtime deadlines and locally generated timestamps; not a
/// trusted estimate of UTC and never comparable to another device's clock.
pub fn now_ms() -> i64 {
    static CLOCK: OnceLock<RuntimeClock> = OnceLock::new();
    CLOCK
        .get_or_init(|| {
            let ms = match SystemTime::now().duration_since(UNIX_EPOCH) {
                Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
                Err(e) => -i64::try_from(e.duration().as_millis()).unwrap_or(i64::MAX),
            };
            // Leave ample representational headroom for elapsed time. An
            // initial label at chrono MAX would otherwise freeze DateTime
            // projections as soon as the process clock advanced.
            RuntimeClock::new(ms.clamp(-8_000_000_000_000, 8_000_000_000_000))
        })
        .now_ms()
}

pub fn now() -> DateTime<Utc> {
    DateTime::from_timestamp_millis(now_ms()).unwrap_or(DateTime::<Utc>::MAX_UTC)
}

/// Freshness is established by observing a changed session, not by comparing
/// the host's wall clock to ours. Replayed identical rows never renew it.
#[derive(Default)]
pub struct SessionFreshness {
    observed: HashMap<String, SessionObservation>,
}

struct SessionObservation {
    source: Session,
    seen: Instant,
    received_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
}

impl SessionFreshness {
    pub fn project(&mut self, sessions: &mut [Session], now: DateTime<Utc>, tick: Instant) {
        {
            let ids: std::collections::HashSet<_> =
                sessions.iter().map(|s| s.chat_id.as_str()).collect();
            self.observed.retain(|id, _| ids.contains(id.as_str()));
        }
        for session in sessions {
            let entry = self
                .observed
                .entry(session.chat_id.clone())
                .or_insert_with(|| SessionObservation {
                    source: session.clone(),
                    seen: tick,
                    received_at: now,
                    started_at: project_start(session, now),
                });
            if entry.source != *session {
                if entry.source.started_at != session.started_at
                    || entry.source.device_id != session.device_id
                {
                    entry.started_at = project_start(session, now);
                }
                entry.source = session.clone();
                entry.seen = tick;
                entry.received_at = now;
            }
            if matches!(
                session.status,
                SessionStatus::Working | SessionStatus::AwaitingInput
            ) {
                session.updated_at = entry.received_at;
                session.started_at = entry.started_at;
                if tick.saturating_duration_since(entry.seen)
                    > Duration::from_millis(crate::view::SESSION_STALE_MS as u64)
                {
                    // A new downstream subscriber must not revive a replayed,
                    // expired live row. The durable source remains untouched.
                    session.status = SessionStatus::Idle;
                    session.started_at = None;
                }
            }
        }
    }
}

/// Rebase engine presence watches into the UI process's clock domain. Engine
/// and UI can start on opposite sides of a civil clock correction.
#[derive(Default)]
pub struct DeviceFreshness {
    observed: HashMap<String, (DateTime<Utc>, DateTime<Utc>)>,
}

impl DeviceFreshness {
    pub fn project(&mut self, devices: &mut [Device], now: DateTime<Utc>) {
        let ids: std::collections::HashSet<_> = devices.iter().map(|d| d.id.as_str()).collect();
        self.observed.retain(|id, _| ids.contains(id.as_str()));
        for device in devices {
            let Some(source) = device.last_seen_at else {
                self.observed.remove(&device.id);
                continue;
            };
            let entry = self
                .observed
                .entry(device.id.clone())
                .or_insert((source, now));
            if entry.0 != source {
                *entry = (source, now);
            }
            device.last_seen_at = Some(entry.1);
        }
    }
}

fn project_start(session: &Session, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let started = session.started_at?;
    let elapsed = session
        .updated_at
        .signed_duration_since(started)
        .max(chrono::TimeDelta::zero());
    now.checked_sub_signed(elapsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_clock_handles_wrong_epochs_and_extreme_elapsed_time() {
        for epoch in [i64::MIN, -1, 0, 1, i64::MAX] {
            let clock = RuntimeClock::new(epoch);
            assert!(clock.at_elapsed(Duration::from_secs(60)) >= epoch);
            assert!(clock.at_elapsed(Duration::MAX) >= epoch);
        }
    }

    #[test]
    fn device_watches_translate_clock_domains_without_renewing_replays() {
        let mut tracker = DeviceFreshness::default();
        let mut source: Device = serde_json::from_value(serde_json::json!({"id":"peer", "name":"Peer", "platform":"linux", "lastSeenAt": DateTime::<Utc>::MAX_UTC})).unwrap();
        let now = DateTime::<Utc>::UNIX_EPOCH;
        let mut devices = vec![source.clone()];
        tracker.project(&mut devices, now);
        assert_eq!(devices[0].last_seen_at, Some(now));
        devices = vec![source.clone()];
        tracker.project(&mut devices, now + chrono::TimeDelta::seconds(90));
        assert_eq!(devices[0].last_seen_at, Some(now));
        source.last_seen_at = Some(DateTime::<Utc>::MIN_UTC);
        devices = vec![source];
        tracker.project(&mut devices, now + chrono::TimeDelta::seconds(91));
        assert_eq!(
            devices[0].last_seen_at,
            Some(now + chrono::TimeDelta::seconds(91))
        );
    }

    #[test]
    fn receipt_freshness_expires_despite_wrong_clock_and_replays() {
        let tick = Instant::now();
        let now = DateTime::<Utc>::UNIX_EPOCH;
        for remote in [DateTime::<Utc>::MIN_UTC, DateTime::<Utc>::MAX_UTC] {
            let row = Session {
                chat_id: "chat".into(),
                device_id: "remote".into(),
                status: SessionStatus::Working,
                started_at: None,
                updated_at: remote,
                last_completed_turn: None,
            };
            let mut freshness = SessionFreshness::default();
            let mut rows = vec![row.clone()];
            freshness.project(&mut rows, now, tick);
            assert_eq!(
                crate::view::effective_indicator(rows.first(), now),
                crate::view::Indicator::Working
            );
            let later = tick + Duration::from_secs(46);
            rows = vec![row];
            // The local clock can also jump to an arbitrary date. Projection
            // still uses elapsed receipt time, never either clock's offset.
            freshness.project(&mut rows, now + chrono::TimeDelta::days(9000), later);
            assert_eq!(
                crate::view::effective_indicator(rows.first(), now + chrono::TimeDelta::days(9000)),
                crate::view::Indicator::None
            );
        }
    }
}
