//! Connectivity truth — the phone edition of the engine's
//! `compute_connectivity` + DegradeGrace, and the send-state derivation the
//! row badges and composer notice render (legacy `Connectivity.swift`).
//!
//! One graced posture feeds every consumer: a source must be RAW-degraded for
//! [`DEGRADE_GRACE_MS`] continuously before it reports degraded (sub-second
//! blips — room joins on navigation, idle-link wakes — never flash the UI),
//! while recovery reports instantly (hide-fast, show-slow). Sources keep
//! independent timers: the OS path, the registry room, each open chat room.

use std::collections::{BTreeSet, HashMap};

/// doc_host.rs DEGRADE_GRACE.
pub const DEGRADE_GRACE_MS: i64 = 4_000;
/// state.rs UNDELIVERED_GRACE_MS: a send unadopted past this is explicitly
/// Failed (with a retry affordance), never a silent forever-spinner.
pub const UNDELIVERED_GRACE_MS: i64 = 120_000;
/// Presence freshness: a device whose newest heartbeat is older than this
/// reads offline (workspace_host.rs presence TTL).
pub const PRESENCE_FRESH_MS: i64 = 45_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ConnectivityState {
    /// Demo mode / no edge transports — hide the pill.
    #[default]
    Disabled,
    /// The OS reports no network path (graced): sends are saved locally.
    Offline,
    /// The registry room is down (graced) while the path looks fine.
    Reconnecting,
    Connected,
}

/// The graced posture every surface renders.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Connectivity {
    pub state: ConnectivityState,
    /// Epoch ms of the next scheduled registry/chat redial (countdown), if any.
    pub retry_at_ms: Option<i64>,
    /// The failure that started the current outage (sticky until rejoin).
    pub last_failure: Option<String>,
    /// Open chats whose room is graced-degraded.
    pub degraded_chats: Vec<String>,
}

/// A pending send's user-visible truth. `Failed` wins over `Queued` in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SendState {
    /// In flight on a healthy path.
    Sending,
    /// Pending on a degraded path (offline / room down / host dark) — it is
    /// durable in the doc and will deliver when the path returns.
    Queued,
    /// Unadopted past [`UNDELIVERED_GRACE_MS`] or rejected/expired by the
    /// host: show "Not delivered — retry".
    Failed,
}

/// Raw inputs sampled each recompute.
#[derive(Debug, Clone, Default)]
pub(crate) struct RawConnectivity {
    pub path_offline: bool,
    pub registry_connected: bool,
    pub registry_retry_at_ms: Option<i64>,
    pub last_failure: Option<String>,
    /// (chat id, connected, retry_at_ms) for every chat room that has dialed.
    pub chat_rooms: Vec<(String, bool, Option<i64>)>,
}

/// DegradeGrace bookkeeping: source key → first raw-degraded sample (ms).
#[derive(Debug, Default)]
pub(crate) struct ConnectivityTracker {
    degraded_since: HashMap<String, i64>,
}

impl ConnectivityTracker {
    fn graced(&mut self, key: &str, raw_degraded: bool, now: i64) -> bool {
        if !raw_degraded {
            self.degraded_since.remove(key);
            return false;
        }
        let since = *self.degraded_since.entry(key.to_owned()).or_insert(now);
        now - since >= DEGRADE_GRACE_MS
    }

    /// Anything still inside its grace window (the ticker keeps sampling).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn settling(&self) -> bool {
        !self.degraded_since.is_empty()
    }

    pub fn compute(&mut self, raw: &RawConnectivity, now: i64) -> Connectivity {
        let offline = self.graced("os", raw.path_offline, now);
        let registry_down = self.graced("registry", !raw.registry_connected, now);
        let mut degraded = BTreeSet::new();
        let mut live_keys: Vec<String> = vec!["os".into(), "registry".into()];
        let mut retry_at = raw.registry_retry_at_ms;
        for (chat_id, connected, chat_retry) in &raw.chat_rooms {
            let key = format!("chat:{chat_id}");
            if self.graced(&key, !connected, now) {
                degraded.insert(chat_id.clone());
            }
            live_keys.push(key);
            if let Some(at) = chat_retry
                && retry_at.is_none_or(|current| *at < current)
            {
                retry_at = Some(*at);
            }
        }
        self.degraded_since.retain(|key, _| live_keys.contains(key));
        let state = if offline {
            ConnectivityState::Offline
        } else if registry_down {
            ConnectivityState::Reconnecting
        } else {
            ConnectivityState::Connected
        };
        Connectivity {
            state,
            retry_at_ms: retry_at.filter(|_| state != ConnectivityState::Connected),
            last_failure: raw
                .last_failure
                .clone()
                .filter(|_| state != ConnectivityState::Connected),
            degraded_chats: degraded.into_iter().collect(),
        }
    }
}

/// Oldest-pending-send truth (legacy `AppModel.sendState`).
pub(crate) fn send_state(started_ms: i64, degraded: bool, dead: bool, now: i64) -> SendState {
    if dead || now - started_ms > UNDELIVERED_GRACE_MS {
        SendState::Failed
    } else if degraded {
        SendState::Queued
    } else {
        SendState::Sending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degradation_is_graced_but_recovery_is_instant() {
        let mut tracker = ConnectivityTracker::default();
        let raw = RawConnectivity {
            path_offline: true,
            registry_connected: true,
            ..Default::default()
        };
        assert_eq!(tracker.compute(&raw, 0).state, ConnectivityState::Connected);
        assert_eq!(
            tracker.compute(&raw, DEGRADE_GRACE_MS - 1).state,
            ConnectivityState::Connected
        );
        assert_eq!(
            tracker.compute(&raw, DEGRADE_GRACE_MS).state,
            ConnectivityState::Offline
        );
        let healthy = RawConnectivity {
            registry_connected: true,
            ..Default::default()
        };
        assert_eq!(
            tracker.compute(&healthy, DEGRADE_GRACE_MS + 1).state,
            ConnectivityState::Connected
        );
        assert!(!tracker.settling());
    }

    #[test]
    fn send_state_prefers_failed() {
        assert_eq!(send_state(0, false, false, 10), SendState::Sending);
        assert_eq!(send_state(0, true, false, 10), SendState::Queued);
        assert_eq!(
            send_state(0, true, false, UNDELIVERED_GRACE_MS + 1),
            SendState::Failed
        );
        assert_eq!(send_state(0, false, true, 10), SendState::Failed);
    }
}
