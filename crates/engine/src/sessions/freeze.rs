//! The run task's side of a live-update freeze, and the fold state it hands
//! over (see `handoff.rs` for the engine side, and `docs/live-update.md`,
//! "Agent runs").
//!
//! Compiled everywhere because `drive_run` is: only unix engines ever send
//! an [`EngineFreeze`] or start a run in [`RunMode::Adopt`].

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use zeron_doc::MessagePart;
use zeron_harness::HarnessHandoff;
use zeron_proto::RunRequest;

/// The engine asks a run task to freeze. The task forwards the request to
/// the harness, drains what the harness emitted before it stopped, flushes
/// its fold into the docs and answers with a [`RunFrozen`] — or a reason it
/// is busy.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) struct EngineFreeze {
    pub reply: oneshot::Sender<Result<RunFrozen, String>>,
}

/// A run stopped at a safe point, fold flushed.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) struct RunFrozen {
    pub harness: HarnessHandoff,
    pub fold: FoldSnapshot,
    /// The request the run was started (or adopted) with.
    pub request: RunRequest,
    /// Sending commits (the harness gives its child up and ends its stream,
    /// which the run task then treats as a clean frozen end); dropping thaws
    /// (both carry on as if nothing happened).
    pub verdict: oneshot::Sender<()>,
}

/// How a run task starts.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) enum RunMode {
    /// Spawn the agent (`Harness::run`).
    Start,
    /// Take over a running agent a predecessor handed over (`Harness::adopt`),
    /// continuing its fold.
    Adopt(Box<AdoptSeed>),
}

/// Descriptors an engine keeps open for an adopting harness (nothing on
/// platforms without handoff).
#[cfg(unix)]
pub(crate) type HeldFds = Vec<std::os::fd::OwnedFd>;
#[cfg(not(unix))]
pub(crate) type HeldFds = Vec<()>;

#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) struct AdoptSeed {
    pub harness: HarnessHandoff,
    pub fold: FoldSnapshot,
    /// Engine-owned duplicates of the descriptors `harness` names, made when the
    /// run was registered (before the adoption commits and closes the inherited
    /// originals) and closed once the harness has taken its own copies. The
    /// harness may be delayed (an execution lease waits on a harness update),
    /// and must not find its descriptors closed under it.
    pub fds: HeldFds,
}

/// A streaming doc entry already begun: where it sits and what it holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WrittenSegment {
    pub entry_index: usize,
    pub written: Vec<MessagePart>,
}

/// A live subagent transcript sink (its doc is reopened by id).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentSnapshot {
    pub parent_tool_use_id: String,
    pub doc_id: String,
    pub entry_id: String,
    pub started_at: i64,
    pub segment: Option<WrittenSegment>,
    pub folded: Vec<MessagePart>,
}

/// The run task's in-memory fold at a freeze: everything `drive_run` keeps
/// in locals that the transcript alone does not tell. Taken after a flush,
/// so the docs already hold every folded part.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FoldSnapshot {
    /// The open assistant segment's parts (tool inputs unstripped).
    pub folded: Vec<MessagePart>,
    pub entry_id: String,
    pub segment_started: i64,
    pub segment: Option<WrittenSegment>,
    pub seen_tools: Vec<String>,
    pub seen_images: Vec<String>,
    pub settled_subagents: Vec<String>,
    pub subagents: Vec<SubagentSnapshot>,
    /// Parked between turns for this long (`idle_since`).
    pub parked_ms: Option<u64>,
    /// Since the last tagged subagent event.
    pub subagent_quiet_ms: Option<u64>,
    pub saw_session_started: bool,
    pub self_continued_turn: bool,
    /// The run's own prompt carried a side chat's copied history.
    pub carried_history: bool,
    pub user_message_id: String,
    pub resume_injected: bool,
    pub startup_retry: bool,
}

/// `Some(now - elapsed)`, the inverse of how the snapshot measured it.
pub(crate) fn instant_before(ms: Option<u64>) -> Option<tokio::time::Instant> {
    let now = tokio::time::Instant::now();
    ms.map(|ms| now.checked_sub(Duration::from_millis(ms)).unwrap_or(now))
}

pub(crate) fn elapsed_ms(at: Option<tokio::time::Instant>) -> Option<u64> {
    at.map(|at| u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX))
}

/// Await an optional receiver; never resolves when there is none.
pub(crate) async fn recv_some<T>(
    rx: &mut Option<oneshot::Receiver<T>>,
) -> Result<T, oneshot::error::RecvError> {
    match rx.as_mut() {
        Some(rx) => rx.await,
        None => std::future::pending().await,
    }
}
