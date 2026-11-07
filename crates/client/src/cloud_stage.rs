//! What a Cloud machine says it is doing while a message waits for it.
//!
//! A machine woken by a message may install a release before it answers
//! (`CloudSession::stage` reads [`zeron_proto::CLOUD_STAGE_UPDATING`]). Its
//! heartbeat is back by then, so it doesn't read as starting: only the edge's
//! session list says so. While one of this phone's sends waits on a Cloud
//! machine, the list is read every [`POLL_MS`], and for [`WATCH_MS`] after
//! the machine last read as starting, waking or updating: it says it updates
//! only once its engine is up. Nothing waiting, nothing read.
//!
//! The list also says when a machine rebuilt its chat store from the server
//! copy (`store_recovered_at`): its chats show a quiet note until dismissed.
//! Opening a Cloud chat reads the list at most every [`GLANCE_MS`] for it.

use std::collections::{HashMap, HashSet};

use zeron_proto::CloudSessions;

/// How often the session list is read while a send waits on a machine.
pub(crate) const POLL_MS: i64 = 5_000;
/// How long it is still read after the machine last read as getting to the
/// send. Its look for a release takes seconds; this covers it with room.
pub(crate) const WATCH_MS: i64 = 60_000;
/// Opening a Cloud chat reads the list if it wasn't read this recently.
pub(crate) const GLANCE_MS: i64 = 5 * 60_000;
/// How long after a machine rebuilt its store its chats say so.
pub(crate) const RESTORED_NOTE_MS: i64 = 7 * 24 * 60 * 60_000;

/// One chat with a send waiting on its Cloud machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MachineWait {
    pub chat_id: String,
    /// The machine (session device) it runs on.
    pub device_id: String,
    /// When its oldest waiting send was made.
    pub sent_at_ms: i64,
    /// The machine's heartbeat isn't back: starting or waking.
    pub booting: bool,
}

#[derive(Debug, Default)]
pub(crate) struct CloudStages {
    /// Machines (session devices) that said they are updating, as last read.
    updating: HashSet<String>,
    /// Machines whose wake the edge says is taking far longer than normal.
    slow: HashSet<String>,
    /// Per machine: when it last rebuilt its chat store from the server copy.
    restored: HashMap<String, i64>,
    /// Per machine: the rebuild whose note was dismissed (kept on disk).
    dismissed: HashMap<String, i64>,
    /// Per waiting chat: when its machine last read as getting to the send.
    waited: HashMap<String, i64>,
    last_read_ms: Option<i64>,
    reading: bool,
}

impl CloudStages {
    pub(crate) fn updating(&self, device_id: &str) -> bool {
        self.updating.contains(device_id)
    }

    pub(crate) fn slow(&self, device_id: &str) -> bool {
        self.slow.contains(device_id)
    }

    /// When the machine rebuilt its chat store, while its chats still say so:
    /// recent, and not dismissed.
    pub(crate) fn restored(&self, device_id: &str, now: i64) -> Option<i64> {
        let at = *self.restored.get(device_id)?;
        let dismissed = self.dismissed.get(device_id).is_some_and(|d| *d >= at);
        (!dismissed && now - at <= RESTORED_NOTE_MS).then_some(at)
    }

    /// Stop saying the machine rebuilt its store (until it does again). The
    /// dismissed rebuilds to keep, or `None` when nothing changed.
    pub(crate) fn dismiss_restored(&mut self, device_id: &str) -> Option<HashMap<String, i64>> {
        let at = *self.restored.get(device_id)?;
        if self.dismissed.get(device_id).is_some_and(|d| *d >= at) {
            return None;
        }
        self.dismissed.insert(device_id.to_owned(), at);
        Some(self.dismissed.clone())
    }

    /// Rebuilds dismissed before (from disk).
    pub(crate) fn load_dismissed(&mut self, dismissed: HashMap<String, i64>) {
        self.dismissed = dismissed;
    }

    /// A Cloud chat opened: whether to read the list for what it says about
    /// its machine (counts as being read until [`Self::read_done`]).
    pub(crate) fn wants_glance(&mut self, now: i64) -> bool {
        if self.reading || self.last_read_ms.is_some_and(|at| now - at < GLANCE_MS) {
            return false;
        }
        self.reading = true;
        self.last_read_ms = Some(now);
        true
    }

    /// Note what the sends wait on now; whether to read the list again (and
    /// if so, it counts as being read until [`Self::read_done`]).
    pub(crate) fn wants_read(&mut self, waits: &[MachineWait], now: i64) -> bool {
        self.waited
            .retain(|chat, _| waits.iter().any(|w| &w.chat_id == chat));
        if waits.is_empty() {
            // Nothing shows it, and it would be stale by the next send.
            self.updating.clear();
            self.slow.clear();
        }
        let mut watching = false;
        for wait in waits {
            let held = wait.booting || self.updating(&wait.device_id) || self.slow(&wait.device_id);
            let waited = self
                .waited
                .entry(wait.chat_id.clone())
                .or_insert(wait.sent_at_ms);
            if held {
                *waited = now;
            }
            watching |= now - *waited <= WATCH_MS;
        }
        if !watching || self.reading || self.last_read_ms.is_some_and(|at| now - at < POLL_MS) {
            return false;
        }
        self.reading = true;
        self.last_read_ms = Some(now);
        true
    }

    /// A read of the session list is over (it may have failed). Whether the
    /// machines that say they are updating changed.
    pub(crate) fn read_done(&mut self, sessions: Option<&CloudSessions>) -> bool {
        self.reading = false;
        self.record(sessions)
    }

    /// Any read of the session list (the poll's or a screen's).
    pub(crate) fn record(&mut self, sessions: Option<&CloudSessions>) -> bool {
        let Some(sessions) = sessions else {
            return false;
        };
        let updating: HashSet<String> = sessions
            .sessions
            .iter()
            .filter(|s| s.stage.as_deref() == Some(zeron_proto::CLOUD_STAGE_UPDATING))
            .map(|s| s.device_id.clone())
            .collect();
        let slow: HashSet<String> = sessions
            .sessions
            .iter()
            .filter(|s| s.stage.as_deref() == Some(zeron_proto::CLOUD_STAGE_SLOW))
            .map(|s| s.device_id.clone())
            .collect();
        let restored: HashMap<String, i64> = sessions
            .sessions
            .iter()
            .filter_map(|s| Some((s.device_id.clone(), s.store_recovered_at? as i64)))
            .collect();
        let changed = updating != self.updating || slow != self.slow || restored != self.restored;
        self.updating = updating;
        self.slow = slow;
        self.restored = restored;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::{CloudSession, CloudState};

    fn wait(booting: bool) -> MachineWait {
        MachineWait {
            chat_id: "c".into(),
            device_id: "cloud-s1".into(),
            sent_at_ms: 0,
            booting,
        }
    }

    fn restored_at(at: Option<f64>) -> CloudSessions {
        let mut list = sessions(None);
        list.sessions[0].store_recovered_at = at;
        list
    }

    #[test]
    fn a_machine_that_rebuilt_its_store_says_so_until_dismissed() {
        let mut stages = CloudStages::default();
        // An older edge says nothing about it: no note.
        assert!(!stages.record(Some(&sessions(None))));
        assert_eq!(stages.restored("cloud-s1", 0), None);
        // Rebuilt at 1 000: its chats say so.
        assert!(stages.record(Some(&restored_at(Some(1_000.0)))));
        assert_eq!(stages.restored("cloud-s1", 2_000), Some(1_000));
        assert_eq!(stages.restored("cloud-other", 2_000), None);
        // Dismissed: gone, and dismissing again changes nothing.
        let kept = stages.dismiss_restored("cloud-s1").expect("dismissed");
        assert_eq!(kept.get("cloud-s1"), Some(&1_000));
        assert_eq!(stages.restored("cloud-s1", 2_000), None);
        assert!(stages.dismiss_restored("cloud-s1").is_none());
        // A later rebuild says so again.
        assert!(stages.record(Some(&restored_at(Some(5_000.0)))));
        assert_eq!(stages.restored("cloud-s1", 6_000), Some(5_000));
        // A dismissal kept from before still holds after a restart.
        let mut restarted = CloudStages::default();
        restarted.load_dismissed(kept);
        restarted.record(Some(&restored_at(Some(1_000.0))));
        assert_eq!(restarted.restored("cloud-s1", 2_000), None);
        // An old rebuild isn't news.
        let mut stages = CloudStages::default();
        stages.record(Some(&restored_at(Some(0.0))));
        assert_eq!(stages.restored("cloud-s1", RESTORED_NOTE_MS + 1), None);
    }

    #[test]
    fn opening_a_cloud_chat_glances_at_the_list_now_and_then() {
        let mut stages = CloudStages::default();
        assert!(stages.wants_glance(0));
        assert!(!stages.wants_glance(1_000), "still reading");
        stages.read_done(None);
        assert!(!stages.wants_glance(1_000), "just read");
        assert!(stages.wants_glance(GLANCE_MS + 1));
    }

    fn sessions(stage: Option<&str>) -> CloudSessions {
        CloudSessions {
            sessions: vec![CloudSession {
                chat_id: "c".into(),
                device_id: "cloud-s1".into(),
                space_id: "sp".into(),
                state: CloudState::Ready,
                stage: stage.map(String::from),
                ..Default::default()
            }],
            available: true,
        }
    }

    /// One poll step at `now`: read if it wants to, answering with `stage`.
    fn poll(
        stages: &mut CloudStages,
        waits: &[MachineWait],
        now: i64,
        stage: Option<&str>,
    ) -> bool {
        let read = stages.wants_read(waits, now);
        if read {
            stages.read_done(Some(&sessions(stage)));
        }
        read
    }

    #[test]
    fn nothing_waiting_reads_nothing() {
        let mut stages = CloudStages::default();
        assert!(!stages.wants_read(&[], 0));
        assert!(!stages.wants_read(&[], 100_000));
    }

    #[test]
    fn a_woken_machine_is_followed_until_it_says_whether_it_updates() {
        let mut stages = CloudStages::default();
        // Waking, longer than the watch: followed throughout, every POLL_MS.
        assert!(poll(&mut stages, &[wait(true)], 1_000, None));
        assert!(
            !poll(&mut stages, &[wait(true)], 3_000, None),
            "not before POLL_MS"
        );
        assert!(poll(&mut stages, &[wait(true)], 90_000, None));
        // Its heartbeat is back: it no longer reads as booting, but it only
        // now looks for a release. Still read.
        assert!(poll(
            &mut stages,
            &[wait(false)],
            95_000,
            Some(zeron_proto::CLOUD_STAGE_UPDATING)
        ));
        assert!(stages.updating("cloud-s1"));
        // Installing takes a while: followed as long as it says so.
        assert!(poll(
            &mut stages,
            &[wait(false)],
            200_000,
            Some(zeron_proto::CLOUD_STAGE_UPDATING)
        ));
        assert!(poll(&mut stages, &[wait(false)], 250_000, None));
        assert!(!stages.updating("cloud-s1"));
        // Done: a little longer, then nothing more to learn.
        assert!(poll(&mut stages, &[wait(false)], 255_000, None));
        assert!(!poll(
            &mut stages,
            &[wait(false)],
            250_000 + WATCH_MS + 1,
            None
        ));
    }

    #[test]
    fn a_send_that_stops_waiting_stops_the_reads() {
        let mut stages = CloudStages::default();
        assert!(poll(&mut stages, &[wait(false)], 1_000, None));
        // Never taken: the reads end once the watch is over, not never.
        assert!(!stages.wants_read(&[wait(false)], WATCH_MS + 1));
        // Taken (or failed): nothing waits, nothing is read.
        let mut stages = CloudStages::default();
        assert!(poll(&mut stages, &[wait(true)], 1_000, None));
        assert!(!stages.wants_read(&[], 10_000));
        // And what it last said is forgotten: the next send reads it afresh.
        stages.record(Some(&sessions(Some(zeron_proto::CLOUD_STAGE_UPDATING))));
        assert!(stages.updating("cloud-s1"));
        assert!(!stages.wants_read(&[], 20_000));
        assert!(!stages.updating("cloud-s1"));
    }

    #[test]
    fn a_wake_the_edge_says_is_slow_is_noted_and_followed() {
        let mut stages = CloudStages::default();
        assert!(poll(&mut stages, &[wait(true)], 1_000, Some(zeron_proto::CLOUD_STAGE_SLOW)));
        assert!(stages.slow("cloud-s1"));
        assert!(!stages.updating("cloud-s1"));
        // Its heartbeat back but still slow (no engine yet): still followed.
        assert!(poll(&mut stages, &[wait(false)], 90_000, Some(zeron_proto::CLOUD_STAGE_SLOW)));
        assert!(poll(&mut stages, &[wait(false)], 95_000, None));
        assert!(!stages.slow("cloud-s1"));
    }

    #[test]
    fn one_read_at_a_time() {
        let mut stages = CloudStages::default();
        assert!(stages.wants_read(&[wait(true)], 0));
        assert!(!stages.wants_read(&[wait(true)], 10_000), "still reading");
        // A failed read changes nothing, and the next one goes ahead.
        assert!(!stages.read_done(None));
        assert!(stages.wants_read(&[wait(true)], 10_000));
    }

    #[test]
    fn any_read_of_the_list_counts() {
        let mut stages = CloudStages::default();
        assert!(stages.record(Some(&sessions(Some(zeron_proto::CLOUD_STAGE_UPDATING)))));
        assert!(stages.updating("cloud-s1"));
        assert!(!stages.record(Some(&sessions(Some(zeron_proto::CLOUD_STAGE_UPDATING)))));
        // A stage from a newer edge is just not this one.
        assert!(stages.record(Some(&sessions(Some("something-new")))));
        assert!(!stages.updating("cloud-s1"));
    }
}
