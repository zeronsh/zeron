//! What agents (and the person) say to each other, as this engine sees it:
//! messages, reports, asks, answers, routes and spawns, for the live map and
//! the comms feed (`WatchComms`). Ephemeral: the newest [`RING`] in memory.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use chrono::Utc;
use tokio::sync::broadcast;
use zeron_proto::orchestration::{CommEvent, CommKind};

/// Events kept for a new subscriber.
pub const RING: usize = 100;

/// "you" in `CommEvent::from`/`to`: the person.
pub const YOU: &str = "you";

#[derive(Clone)]
pub struct CommBus {
    ring: Arc<Mutex<VecDeque<CommEvent>>>,
    tx: broadcast::Sender<CommEvent>,
}

impl Default for CommBus {
    fn default() -> Self {
        Self::new()
    }
}

impl CommBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(RING);
        Self {
            ring: Arc::new(Mutex::new(VecDeque::with_capacity(RING))),
            tx,
        }
    }

    /// Record and broadcast one event.
    pub fn emit(&self, event: CommEvent) {
        let mut ring = self.ring.lock().unwrap_or_else(PoisonError::into_inner);
        if ring.len() == RING {
            ring.pop_front();
        }
        ring.push_back(event.clone());
        // (Under the ring's lock, so a subscriber's backlog and its live
        // events never overlap or miss one.)
        let _ = self.tx.send(event);
    }

    /// A new event of `kind`, stamped now, emitted.
    pub fn say(&self, kind: CommKind, from: &str, to: &str, summary: &str) {
        self.emit(CommEvent {
            id: uuid::Uuid::new_v4().simple().to_string(),
            kind,
            from: from.to_owned(),
            to: to.to_owned(),
            summary: super::excerpt(summary, 140),
            at: Utc::now(),
        });
    }

    /// The newest events, oldest first.
    pub fn recent(&self) -> Vec<CommEvent> {
        self.ring
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    /// Live events from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<CommEvent> {
        self.tx.subscribe()
    }

    /// The backlog and the live events after it, with none lost between.
    pub fn subscribe_with_recent(&self) -> (Vec<CommEvent>, broadcast::Receiver<CommEvent>) {
        let ring = self.ring.lock().unwrap_or_else(PoisonError::into_inner);
        (ring.iter().cloned().collect(), self.tx.subscribe())
    }
}

/// `WatchComms`: every frame a JSON array of events, oldest first; the first
/// is the backlog (≤ [`RING`]), each later one what happened since. A
/// subscriber that falls behind gets the whole ring again (clients key
/// events by id).
pub(crate) fn stream(bus: &CommBus) -> futures::stream::BoxStream<'static, serde_json::Value> {
    use futures::StreamExt;
    use tokio::sync::broadcast::error::RecvError;
    let (backlog, rx) = bus.subscribe_with_recent();
    let bus = bus.clone();
    futures::stream::unfold((Some(backlog), rx, bus), |(backlog, mut rx, bus)| async move {
        if let Some(backlog) = backlog {
            let value = serde_json::to_value(&backlog).ok()?;
            return Some((value, (None, rx, bus)));
        }
        let events = match rx.recv().await {
            Ok(event) => {
                let mut events = vec![event];
                while let Ok(more) = rx.try_recv() {
                    events.push(more);
                }
                events
            }
            Err(RecvError::Lagged(_)) => bus.recent(),
            Err(RecvError::Closed) => return None,
        };
        let value = serde_json::to_value(&events).ok()?;
        Some((value, (None, rx, bus)))
    })
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_ring_keeps_the_newest_hundred_and_subscribers_miss_nothing() {
        let bus = CommBus::new();
        for i in 0..150 {
            bus.say(CommKind::Message, "a", "b", &format!("m{i}"));
        }
        let (backlog, mut live) = bus.subscribe_with_recent();
        assert_eq!(backlog.len(), RING);
        assert_eq!(backlog[0].summary, "m50");
        assert_eq!(backlog[RING - 1].summary, "m149");
        bus.say(CommKind::Report, "c", YOU, "done");
        assert_eq!(live.recv().await.unwrap().summary, "done");
    }
}
