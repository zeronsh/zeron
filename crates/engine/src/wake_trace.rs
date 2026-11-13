//! When this boot of a Cloud session machine reached each stage on the way to
//! the first run's output, reported once to the edge, which joins it with its
//! side of the wake (`edge/src/cloud/wake.ts`) for an operator to read. On for
//! Cloud runners only: everywhere else [`mark`] is one atomic load.
//!
//! The stages run from outside this process in: the machine's kernel (how long
//! the provider's restore took to reach it), `exec` of this binary (systemd,
//! then the lazy disk loading it), then the engine's own road to the run.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// `Engine::run` began: the binary is loaded and running.
    EngineRun,
    /// The look for a newer release answered (`wake_update`).
    UpdateChecked,
    /// There is one, and it's on the disk: restarting into it.
    UpdateRestart,
    /// The wait for an update ended, on whichever version: work may start.
    Released,
    /// The first runner token (the edge reads this as "online").
    Token,
    /// The host relay joined the device room.
    Relay,
    /// The first nudge arrived.
    Nudge,
    /// The first prompt was picked out of the chat's queue.
    Command,
    /// Credentials ready, the first agent CLI being started.
    AgentSpawn,
    /// The agent CLI reported its session: it is up.
    AgentStarted,
    /// The agent's first text, thought or tool call.
    FirstOutput,
}

#[derive(Default, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Timeline {
    /// The version that answered, and the one it updated to on the way, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_to: Option<String>,
    /// How the wake's look for a release ended (`wake_update::gate`).
    #[serde(skip_serializing_if = "Option::is_none")]
    update_result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    update_checked: Option<i64>,
    /// It found a release and began installing it.
    #[serde(skip_serializing_if = "Option::is_none")]
    update_started: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    update_restart: Option<i64>,
    /// The engine after that restart began.
    #[serde(skip_serializing_if = "Option::is_none")]
    resumed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    released: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sent: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    machine_boot: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    process_start: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    engine_run: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    relay: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nudge: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_spawn: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_started: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_output: Option<i64>,
}

impl Timeline {
    const fn new() -> Self {
        Self {
            version: None,
            updated_from: None,
            updated_to: None,
            update_result: None,
            update_checked: None,
            update_started: None,
            update_restart: None,
            resumed: None,
            released: None,
            sent: None,
            machine_boot: None,
            process_start: None,
            engine_run: None,
            token: None,
            relay: None,
            nudge: None,
            command: None,
            agent_spawn: None,
            agent_started: None,
            first_output: None,
        }
    }

    /// First wins: a stage is when this boot first got there.
    fn mark(&mut self, stage: Stage, now: i64) {
        let slot = match stage {
            Stage::EngineRun => &mut self.engine_run,
            Stage::UpdateChecked => &mut self.update_checked,
            Stage::UpdateRestart => &mut self.update_restart,
            Stage::Released => &mut self.released,
            Stage::Token => &mut self.token,
            Stage::Relay => &mut self.relay,
            Stage::Nudge => &mut self.nudge,
            Stage::Command => &mut self.command,
            Stage::AgentSpawn => &mut self.agent_spawn,
            Stage::AgentStarted => &mut self.agent_started,
            Stage::FirstOutput => &mut self.first_output,
        };
        slot.get_or_insert(now);
    }
}

struct Reporter {
    edge_url: String,
    tokens: Arc<dyn zeron_rpc::TokenSource>,
}

/// Carries a wake's timeline across the restart into an update.
const PERSIST_FILE: &str = "wake-trace.json";
/// A file older than this is some earlier wake's.
const PERSIST_FRESH_MS: i64 = 15 * 60_000;

#[derive(Serialize, Deserialize)]
struct Persisted {
    at: i64,
    timeline: Timeline,
}

struct Trace {
    timeline: Timeline,
    reporter: Option<Reporter>,
    reported: bool,
    data_dir: Option<PathBuf>,
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static TRACE: Mutex<Trace> = Mutex::new(Trace {
    timeline: Timeline::new(),
    reporter: None,
    reported: false,
    data_dir: None,
});

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn with_trace<T>(change: impl FnOnce(&mut Trace) -> T) -> T {
    change(&mut TRACE.lock().unwrap_or_else(PoisonError::into_inner))
}

/// This process is a Cloud runner: start keeping time, carrying on from the wake
/// that restarted into it, if one did.
pub fn enable(data_dir: &Path) {
    let now = now_ms();
    let (machine_boot, process_start) = process_times(now);
    let earlier = read_persisted(data_dir, now);
    with_trace(|trace| {
        trace.data_dir = Some(data_dir.to_path_buf());
        trace.timeline = merged(earlier, machine_boot, process_start, now);
    });
    ENABLED.store(true, Ordering::Release);
    mark(Stage::EngineRun);
}

/// What this start knows: an earlier part of the same wake keeps its times, as the
/// wake began there, and this start is when it resumed.
fn merged(
    earlier: Option<Timeline>,
    machine_boot: Option<i64>,
    process_start: Option<i64>,
    now: i64,
) -> Timeline {
    let version = Some(zeron_update::current_version().to_owned());
    match earlier {
        Some(mut timeline) => {
            timeline.version = version;
            timeline.resumed = Some(now);
            timeline
        }
        None => Timeline {
            version,
            machine_boot,
            process_start,
            ..Timeline::new()
        },
    }
}

/// Left in place until the wait is over ([`forget_persisted`]): a new version that
/// dies on trial hands over to the old one, which is still the same wake.
fn read_persisted(data_dir: &Path, now: i64) -> Option<Timeline> {
    let bytes = std::fs::read(data_dir.join(PERSIST_FILE)).ok()?;
    let persisted: Persisted = serde_json::from_slice(&bytes).ok()?;
    (now - persisted.at < PERSIST_FRESH_MS).then_some(persisted.timeline)
}

/// The wait for an update is over: the next start is another wake.
pub fn forget_persisted() {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    if let Some(data_dir) = with_trace(|trace| trace.data_dir.clone()) {
        let _ = std::fs::remove_file(data_dir.join(PERSIST_FILE));
    }
}

/// Before a restart into an update: the next start carries on from here.
pub fn persist() {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let (data_dir, timeline) = with_trace(|trace| (trace.data_dir.clone(), trace.timeline.clone()));
    let Some(data_dir) = data_dir else {
        return;
    };
    let persisted = Persisted {
        at: now_ms(),
        timeline,
    };
    let written = serde_json::to_vec(&persisted)
        .map_err(std::io::Error::other)
        .and_then(|bytes| std::fs::write(data_dir.join(PERSIST_FILE), bytes));
    if let Err(err) = written {
        tracing::debug!(error = %err, "couldn't save the wake's timeline");
    }
}

/// It found `version` and is installing it.
pub fn note_updating(version: &str) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let now = now_ms();
    with_trace(|trace| {
        trace.timeline.update_started.get_or_insert(now);
        trace.timeline.updated_from = Some(zeron_update::current_version().to_owned());
        trace.timeline.updated_to = Some(version.to_owned());
    });
}

/// How its look for a release ended.
pub fn note_result(result: &str) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    with_trace(|trace| trace.timeline.update_result = Some(result.to_owned()));
}

/// This wake went to install a release.
pub fn was_updating() -> bool {
    ENABLED.load(Ordering::Acquire) && with_trace(|trace| trace.timeline.update_started.is_some())
}

/// The release this very process is, when a wake updated to it.
pub fn updated_to() -> Option<String> {
    if !ENABLED.load(Ordering::Acquire) {
        return None;
    }
    with_trace(|trace| trace.timeline.updated_to.clone())
        .filter(|version| version == zeron_update::current_version())
}

/// Tell the edge how far this wake has got, so a longer one reads as what it is.
/// Best effort, and never in the wake's way.
pub fn announce_stage(stage: Option<&'static str>) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let reporter = with_trace(|trace| {
        trace
            .reporter
            .as_ref()
            .map(|reporter| (reporter.edge_url.clone(), reporter.tokens.clone()))
    });
    if let Some((edge_url, tokens)) = reporter {
        let url = format!("{edge_url}/runner/wake-stage");
        tokio::spawn(send(url, tokens, serde_json::json!({ "stage": stage })));
    }
}

/// Where the report goes, once the runner has its edge and tokens.
pub fn report_to(edge_url: &str, tokens: Arc<dyn zeron_rpc::TokenSource>) {
    let edge_url = edge_url.trim_end_matches('/').to_owned();
    with_trace(|trace| trace.reporter = Some(Reporter { edge_url, tokens }));
}

pub fn mark(stage: Stage) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let now = now_ms();
    with_trace(|trace| trace.timeline.mark(stage, now));
    tracing::info!(?stage, "cloud boot stage");
}

/// The first prompt out of the queue, with the time its sender stamped on it.
pub fn mark_command(sent: i64) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    with_trace(|trace| {
        trace.timeline.sent.get_or_insert(sent);
    });
    mark(Stage::Command);
}

/// An event of a run's agent: its start, then the first real output, which
/// closes this boot's timeline.
pub fn note_agent_event(session_started: bool) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    if session_started {
        mark(Stage::AgentStarted);
        return;
    }
    let report = with_trace(|trace| {
        if trace.reported {
            return None;
        }
        trace.reported = true;
        trace.timeline.mark(Stage::FirstOutput, now_ms());
        let body = trace.timeline.clone();
        trace.reporter.as_ref().map(|reporter| {
            (
                format!("{}/runner/wake-timing", reporter.edge_url),
                reporter.tokens.clone(),
                body,
            )
        })
    });
    if let Some((url, tokens, body)) = report {
        tokio::spawn(send(url, tokens, body));
    }
}

/// Best effort, and never in a run's way.
async fn send(url: String, tokens: Arc<dyn zeron_rpc::TokenSource>, body: impl Serialize) {
    let http = crate::runner::http_client();
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let Ok(token) = tokens.token().await else {
            continue;
        };
        match http.post(&url).bearer_auth(token).json(&body).send().await {
            Ok(res) if res.status().is_success() => return,
            Ok(res) => {
                tracing::debug!(status = res.status().as_u16(), %url, "wake report refused");
                // Too old an edge, or a device it no longer knows: retrying
                // can't change that.
                if res.status().is_client_error() {
                    return;
                }
            }
            Err(err) => tracing::debug!(error = %err, %url, "wake report failed"),
        }
    }
}

/// When the machine's kernel started and when this process did. Linux only;
/// `None` elsewhere or if `/proc` says something unexpected.
fn process_times(now: i64) -> (Option<i64>, Option<i64>) {
    let uptime = std::fs::read_to_string("/proc/uptime").ok();
    let stat = std::fs::read_to_string("/proc/self/stat").ok();
    let boot = uptime
        .as_deref()
        .and_then(|text| machine_boot_at(text, now));
    let start = stat
        .as_deref()
        .zip(boot)
        .and_then(|(text, boot)| process_start_at(text, boot));
    (boot, start)
}

/// `/proc/uptime`: seconds since boot, first field.
fn machine_boot_at(proc_uptime: &str, now: i64) -> Option<i64> {
    let seconds: f64 = proc_uptime.split_whitespace().next()?.parse().ok()?;
    Some(now - (seconds * 1000.0) as i64)
}

/// `/proc/self/stat` field 22: this process's start, in clock ticks after boot
/// (100 per second on every Linux this runs on). The second field is the
/// command in parentheses, which may itself hold spaces or parentheses.
fn process_start_at(proc_stat: &str, machine_boot: i64) -> Option<i64> {
    let after_command = &proc_stat[proc_stat.rfind(')')? + 1..];
    let ticks: i64 = after_command.split_whitespace().nth(19)?.parse().ok()?;
    Some(machine_boot + ticks * 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stage_is_when_this_boot_first_got_there() {
        let mut timeline = Timeline::new();
        timeline.mark(Stage::Token, 5);
        timeline.mark(Stage::Token, 9);
        timeline.mark(Stage::FirstOutput, 7);
        assert_eq!(timeline.token, Some(5));
        assert_eq!(timeline.first_output, Some(7));
    }

    #[test]
    fn reports_camel_case_and_only_what_happened() {
        let mut timeline = Timeline::new();
        timeline.mark(Stage::AgentStarted, 12);
        timeline.machine_boot = Some(3);
        timeline.sent = Some(1);
        assert_eq!(
            serde_json::to_value(&timeline).unwrap(),
            serde_json::json!({ "sent": 1, "machineBoot": 3, "agentStarted": 12 })
        );
    }

    #[test]
    fn a_wake_keeps_its_timeline_across_the_restart_into_an_update() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = Timeline::new();
        first.mark(Stage::Token, 100);
        first.machine_boot = Some(50);
        first.updated_to = Some("9.9.9".into());
        first.update_started = Some(110);
        std::fs::write(
            dir.path().join(PERSIST_FILE),
            serde_json::to_vec(&Persisted {
                at: 1_000,
                timeline: first,
            })
            .unwrap(),
        )
        .unwrap();

        let earlier = read_persisted(dir.path(), 2_000);
        assert!(earlier.is_some());
        // Still there for the old version, if this one dies on trial.
        assert_eq!(read_persisted(dir.path(), 2_100), earlier);
        let merged = merged(earlier, Some(1_900), Some(1_950), 2_000);
        assert_eq!(merged.token, Some(100), "the wake began before the restart");
        assert_eq!(merged.machine_boot, Some(50));
        assert_eq!(merged.resumed, Some(2_000));
        assert_eq!(merged.update_started, Some(110));
        assert_eq!(
            merged.version.as_deref(),
            Some(zeron_update::current_version())
        );

        // Some earlier wake's, left by a restart that never happened.
        std::fs::write(
            dir.path().join(PERSIST_FILE),
            serde_json::to_vec(&Persisted {
                at: 1_000,
                timeline: Timeline::new(),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(read_persisted(dir.path(), 1_000 + PERSIST_FRESH_MS), None);
        assert_eq!(merged_fresh().resumed, None);
    }

    fn merged_fresh() -> Timeline {
        merged(None, Some(1), Some(2), 3)
    }

    #[test]
    fn reads_the_machines_boot_from_its_uptime() {
        assert_eq!(machine_boot_at("12.50 45.10\n", 100_000), Some(87_500));
        assert_eq!(machine_boot_at("", 100_000), None);
        assert_eq!(machine_boot_at("soon 1", 100_000), None);
    }

    #[test]
    fn reads_this_process_start_past_a_command_with_spaces_and_parentheses() {
        // pid (comm) state ppid pgrp session tty_nr tpgid flags minflt cminflt
        // majflt cmajflt utime stime cutime cstime priority nice threads
        // itrealvalue starttime …
        let stat = "42 (zeron (a) b) S 1 42 42 0 -1 4194560 10 0 0 0 1 2 0 0 20 0 8 0 350 12345";
        assert_eq!(process_start_at(stat, 1_000), Some(4_500));
        assert_eq!(process_start_at("garbage", 1_000), None);
        assert_eq!(process_start_at("42 (zeron) S 1 2", 1_000), None);
    }
}
