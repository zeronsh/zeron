//! Stopping one tool call of a live turn — a hung command, a subagent —
//! while the turn goes on, shared between the host and the driver through
//! [`crate::TurnControl`].
//!
//! The host asks with the call's id (the `ToolCall` id in the transcript; a
//! subagent is addressed by the id of the call that spawned it). The driver
//! turns that into its agent's own stop, when the agent has one, and answers
//! what it made of the request. The call itself then ends the agent's way:
//! its result arrives as an error (a killed command, an interrupted
//! subagent) and the agent carries on from it.
//!
//! A driver that never declares support ([`ToolStops::accept`]) is answered
//! [`ToolStop::Unsupported`] at once, so the host never waits on one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

/// How long the host waits for the driver's answer.
const ANSWER_WITHIN: Duration = Duration::from_secs(5);

/// What the driver made of a request to stop one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStop {
    /// The agent was asked to stop it; the call's result follows.
    Stopping,
    /// Nothing of that id is running (it ended meanwhile, or never ran here).
    NotRunning,
    /// This agent can't stop that call alone. The reason is for the person.
    Unsupported(String),
}

impl ToolStop {
    /// The wire spelling (`StopToolCall`'s `outcome`).
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolStop::Stopping => "stopping",
            ToolStop::NotRunning => "notRunning",
            ToolStop::Unsupported(_) => "unsupported",
        }
    }
}

/// One request, handed to the driver; answer it with [`Self::answer`].
/// Dropped unanswered, the host reads it as unsupported.
pub struct ToolStopRequest {
    pub call_id: String,
    reply: oneshot::Sender<ToolStop>,
}

impl ToolStopRequest {
    pub fn answer(self, outcome: ToolStop) {
        let _ = self.reply.send(outcome);
    }
}

pub(crate) struct ToolStops {
    accepted: AtomicBool,
    tx: mpsc::UnboundedSender<ToolStopRequest>,
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<ToolStopRequest>>,
}

impl Default for ToolStops {
    fn default() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            accepted: AtomicBool::new(false),
            tx,
            rx: tokio::sync::Mutex::new(rx),
        }
    }
}

impl ToolStops {
    /// Driver: it answers [`Self::requested`] from now on.
    pub fn accept(&self) {
        self.accepted.store(true, Ordering::Release);
    }

    /// Driver: resolves with each request. Cancel-safe (a `select!` arm).
    pub async fn requested(&self) -> ToolStopRequest {
        let mut rx = self.rx.lock().await;
        match rx.recv().await {
            Some(request) => request,
            // The sender lives in `self`: never closed while we can be polled.
            None => std::future::pending().await,
        }
    }

    /// Host: ask the driver to stop `call_id`, and wait for its answer.
    pub async fn stop(&self, call_id: &str) -> ToolStop {
        if !self.accepted.load(Ordering::Acquire) {
            return ToolStop::Unsupported(String::new());
        }
        let (reply, answer) = oneshot::channel();
        let request = ToolStopRequest {
            call_id: call_id.to_owned(),
            reply,
        };
        if self.tx.send(request).is_err() {
            return ToolStop::NotRunning;
        }
        match tokio::time::timeout(ANSWER_WITHIN, answer).await {
            Ok(Ok(outcome)) => outcome,
            // The runtime ended without reading it: nothing runs to stop.
            Ok(Err(_)) => ToolStop::NotRunning,
            Err(_) => ToolStop::Unsupported("The agent didn't answer the stop.".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_driver_that_never_accepts_is_unsupported_at_once() {
        let stops = ToolStops::default();
        assert!(matches!(stops.stop("call").await, ToolStop::Unsupported(_)));
    }

    #[tokio::test]
    async fn the_driver_answers_each_request() {
        let stops = std::sync::Arc::new(ToolStops::default());
        stops.accept();
        let driver = stops.clone();
        tokio::spawn(async move {
            loop {
                let request = driver.requested().await;
                let outcome = if request.call_id == "live" {
                    ToolStop::Stopping
                } else {
                    ToolStop::NotRunning
                };
                request.answer(outcome);
            }
        });
        assert_eq!(stops.stop("live").await, ToolStop::Stopping);
        assert_eq!(stops.stop("gone").await, ToolStop::NotRunning);
    }

    #[tokio::test]
    async fn a_request_dropped_unanswered_is_not_running() {
        let stops = std::sync::Arc::new(ToolStops::default());
        stops.accept();
        let driver = stops.clone();
        tokio::spawn(async move {
            drop(driver.requested().await);
        });
        assert_eq!(stops.stop("x").await, ToolStop::NotRunning);
    }
}
