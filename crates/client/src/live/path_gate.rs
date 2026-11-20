//! Holds a client's first dials until the platform has said whether there's a network.
//!
//! A phone starts its registry and its warm rooms the moment the client is built, but
//! the OS's first path report (`NWPathMonitor` on iOS) lands a moment later. Launched in
//! airplane mode, a real iPhone made 22 refused dials and pulls in those 120 ms before
//! the report parked everything. Each connect loop now waits for the first report, or
//! [`FIRST_REPORT_PATIENCE`] if none comes, and while the report says offline it waits
//! for the network to come back, with a safety timer so a wrong report can't silence it.

use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;

/// How long a connect waits for the platform's first path report. On an iPhone Air it
/// arrived 111 to 275 ms after the client started (11 launches), and its arrival releases
/// the wait at once, so this only bounds a platform that never reports.
pub(crate) const FIRST_REPORT_PATIENCE: Duration = Duration::from_secs(1);
/// A connect parked on "offline" tries anyway after this: the monitor can be wrong.
pub(crate) const OFFLINE_SAFETY: Duration = Duration::from_secs(30);

/// The network path as the platform last reported it: None until its first report.
#[derive(Clone)]
pub(crate) struct PathGate {
    path: watch::Receiver<Option<bool>>,
    started: Instant,
}

impl PathGate {
    pub(crate) fn new(path: watch::Receiver<Option<bool>>, started: Instant) -> Self {
        Self { path, started }
    }

    /// Resolves when a connect may go: the platform said there's a path, or it hasn't
    /// said anything within [`FIRST_REPORT_PATIENCE`] of the client starting, or it said
    /// there isn't one and [`OFFLINE_SAFETY`] went by.
    pub(crate) async fn ready(&self) {
        let mut path = self.path.clone();
        let unreported = path.borrow().is_none();
        if unreported {
            let first = self.started + FIRST_REPORT_PATIENCE;
            let _ = tokio::time::timeout_at(first, path.wait_for(Option::is_some))
                .await
                .map(|seen| seen.is_ok());
        }
        let offline = *path.borrow() == Some(false);
        if offline {
            let _ = tokio::time::timeout(OFFLINE_SAFETY, path.wait_for(|p| *p == Some(true)))
                .await
                .map(|seen| seen.is_ok());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn waited(gate: &PathGate) -> Duration {
        let start = Instant::now();
        gate.ready().await;
        start.elapsed()
    }

    #[tokio::test(start_paused = true)]
    async fn a_connect_waits_for_the_first_report_and_no_longer() {
        // Reported online straight away: no wait.
        let (tx, rx) = watch::channel(None);
        let gate = PathGate::new(rx, Instant::now());
        tx.send_replace(Some(true));
        assert_eq!(waited(&gate).await, Duration::ZERO);

        // No report yet: until it comes.
        let (tx, rx) = watch::channel(None);
        let gate = PathGate::new(rx, Instant::now());
        let report = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            tx.send_replace(Some(true));
            tx
        });
        assert_eq!(waited(&gate).await, Duration::from_millis(100));
        drop(report.await);

        // A platform that never reports: the patience, then go.
        let (_tx, rx) = watch::channel(None);
        let gate = PathGate::new(rx, Instant::now());
        assert_eq!(waited(&gate).await, FIRST_REPORT_PATIENCE);
    }

    #[tokio::test(start_paused = true)]
    async fn offline_parks_until_the_network_returns_or_the_safety_timer() {
        let (tx, rx) = watch::channel(Some(false));
        let gate = PathGate::new(rx, Instant::now());
        let back = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(7)).await;
            tx.send_replace(Some(true));
            tx
        });
        assert_eq!(waited(&gate).await, Duration::from_secs(7));
        drop(back.await);

        let (_tx, rx) = watch::channel(Some(false));
        let gate = PathGate::new(rx, Instant::now());
        assert_eq!(waited(&gate).await, OFFLINE_SAFETY, "a wrong monitor can't silence it");
    }
}
