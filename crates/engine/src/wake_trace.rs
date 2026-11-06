//! When this boot of a Cloud session machine reached each stage on the way to
//! the first run's output, reported once to the edge, which joins it with its
//! side of the wake (`edge/src/cloud/wake.ts`) for an operator to read. On for
//! Cloud runners only: everywhere else [`mark`] is one atomic load.
//!
//! The stages run from outside this process in: the machine's kernel (how long
//! the provider's restore took to reach it), `exec` of this binary (systemd,
//! then the lazy disk loading it), then the engine's own road to the run.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// `Engine::run` began: the binary is loaded and running.
    EngineRun,
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

#[derive(Default, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Timeline {
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
    url: String,
    tokens: Arc<dyn zeron_rpc::TokenSource>,
}

struct Trace {
    timeline: Timeline,
    reporter: Option<Reporter>,
    reported: bool,
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static TRACE: Mutex<Trace> = Mutex::new(Trace {
    timeline: Timeline::new(),
    reporter: None,
    reported: false,
});

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn with_trace<T>(change: impl FnOnce(&mut Trace) -> T) -> T {
    change(&mut TRACE.lock().unwrap_or_else(PoisonError::into_inner))
}

/// This process is a Cloud runner: start keeping time.
pub fn enable() {
    let now = now_ms();
    let (machine_boot, process_start) = process_times(now);
    with_trace(|trace| {
        trace.timeline.machine_boot = machine_boot;
        trace.timeline.process_start = process_start;
    });
    ENABLED.store(true, Ordering::Release);
    mark(Stage::EngineRun);
}

/// Where the report goes, once the runner has its edge and tokens.
pub fn report_to(edge_url: &str, tokens: Arc<dyn zeron_rpc::TokenSource>) {
    let url = format!("{}/runner/wake-timing", edge_url.trim_end_matches('/'));
    with_trace(|trace| trace.reporter = Some(Reporter { url, tokens }));
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
        trace
            .reporter
            .as_ref()
            .map(|reporter| (reporter.url.clone(), reporter.tokens.clone(), body))
    });
    if let Some((url, tokens, body)) = report {
        tokio::spawn(send(url, tokens, body));
    }
}

/// Best effort, and never in a run's way.
async fn send(url: String, tokens: Arc<dyn zeron_rpc::TokenSource>, body: Timeline) {
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
                tracing::debug!(status = res.status().as_u16(), "wake timing report refused");
                // Too old an edge, or a device it no longer knows: retrying
                // can't change that.
                if res.status().is_client_error() {
                    return;
                }
            }
            Err(err) => tracing::debug!(error = %err, "wake timing report failed"),
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
