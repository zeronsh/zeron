//! S-hibernate: parked ballast sessions, then resume (bench.md 3.4).
//!
//! Each of N target chats and a few warm chats runs one `zb-replay` turn in
//! ballast mode (resident anonymous memory plus idle children, standing in
//! for a Claude process and its MCP servers). `ZERON_SESSION_IDLE_MS` is set
//! on every revision. After the ballast is resident the scenario asks
//! `HibernateIdle` / `HibernateChat` when those RPCs exist, then waits out
//! the idle window. Revisions without hibernation reap the parked child
//! instead (upstream's reaper is 30 minutes unless this env is set). A
//! missing resume is a passing result with an explicit detail: "baseline:
//! cold start, no resume", or "session stayed live". An error result from
//! `--resume` with no state fails the trial.
//!
//! `mode=real` (a seeded Claude JSONL, zero tokens) is specified and not
//! wired. The acceptance run is ballast.
//!
//! `t_resume` / `t_warm` are send → first new transcript frame. `t_*_marker`
//! is send → the replay marker, which is the spec's latency once the new
//! process is speaking.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

use super::driver::{Ui, percentile};
use super::fixture::{
    self, ChatInfo, Fixture, Profile, bad_status, last_turn_has_error_part, last_turn_text,
    turn_settled,
};
use super::{TrialCtx, Workload, load_params};
use crate::ab::Tier;
use crate::mono_now;
use crate::probe::ProbeEvent;
use crate::replay::script::text_sha256;
use crate::rpc::Client;
use crate::sample::proc::parse_smaps_rollup;
use crate::transcript::ChatView;

const CONFIG: &str = include_str!("../../scenarios/hibernate.toml");

/// `outcome` metric: the parked targets stayed up.
const OUTCOME_STAYED: f64 = 0.0;
/// `outcome` metric: the idle reaper ended them (`Hibernate*` missing or refused).
const OUTCOME_REAPED: f64 = 1.0;
/// `outcome` metric: `HibernateIdle` / `HibernateChat` reported a hibernation and the child exited.
const OUTCOME_HIBERNATED: f64 = 2.0;

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct HibernateParams {
    pub mode: String,
    pub chats: usize,
    pub warm_chats: usize,
    pub ballast_mib: u64,
    pub children: String,
    pub idle_ms: u64,
    pub warm_every_ms: u64,
    pub grace_ms: u64,
    pub startup_ms: u64,
    pub resume_ms_per_mb: u64,
    pub ballast_wait_ms: u64,
    pub resume_wait_ms: u64,
}

impl Default for HibernateParams {
    fn default() -> Self {
        Self {
            mode: "ballast".into(),
            chats: 6,
            warm_chats: 2,
            ballast_mib: 300,
            children: "2:250".into(),
            idle_ms: 20_000,
            warm_every_ms: 10_000,
            grace_ms: 8_000,
            startup_ms: 0,
            resume_ms_per_mb: 0,
            ballast_wait_ms: 45_000,
            resume_wait_ms: 60_000,
        }
    }
}

pub struct Hibernate {
    pub p: HibernateParams,
    tier: Tier,
}

impl Hibernate {
    pub fn from_params(tier: Tier, overrides: &[(String, String)]) -> Result<Self> {
        let p: HibernateParams = load_params(CONFIG, tier, overrides)?;
        ensure!(
            p.mode == "ballast",
            "mode must be ballast (real mode, a seeded Claude JSONL, is specified in bench.md 3.4 and is not wired)"
        );
        ensure!(p.chats > 0, "chats must be positive");
        ensure!(p.warm_chats > 0, "warm_chats must be positive");
        ensure!(
            p.warm_every_ms > 0 && p.warm_every_ms < p.idle_ms,
            "warm_every_ms must sit inside the idle window"
        );
        let _ = per_session_mib(p.ballast_mib, &p.children)?;
        Ok(Self { p, tier })
    }
}

impl Workload for Hibernate {
    const ID: &'static str = "S-hibernate";

    fn variant(&self) -> Value {
        json!({
            "mode": self.p.mode,
            "chats": self.p.chats,
            "warm_chats": self.p.warm_chats,
            "ballast_mib": self.p.ballast_mib,
            "children": self.p.children,
            "idle_ms": self.p.idle_ms,
            "tier": self.tier.as_str(),
        })
    }

    fn variant_key(&self) -> String {
        format!("{}-{}-{}", self.p.mode, self.p.chats, self.tier.as_str())
    }

    fn profile(&self) -> Profile {
        Profile::P20
    }

    fn engine_env(&self) -> Vec<(String, String)> {
        vec![
            ("ZB_REPLAY_MODE".into(), "ballast".into()),
            (
                "ZB_REPLAY_BALLAST_MIB".into(),
                self.p.ballast_mib.to_string(),
            ),
            ("ZB_REPLAY_CHILDREN".into(), self.p.children.clone()),
            ("ZB_REPLAY_STARTUP_MS".into(), self.p.startup_ms.to_string()),
            (
                "ZB_REPLAY_RESUME_MS_PER_MB".into(),
                self.p.resume_ms_per_mb.to_string(),
            ),
            ("ZERON_SESSION_IDLE_MS".into(), self.p.idle_ms.to_string()),
        ]
    }

    async fn run(&self, ctx: &mut TrialCtx) -> Result<()> {
        let p = &self.p;
        let fx = ctx.fixture.clone();
        let (n_child, _) = parse_children(&p.children)?;
        let per_mib = per_session_mib(p.ballast_mib, &p.children)?;
        let client = ctx.connect().await?;
        let mut ui = Ui::new(client, ctx.stats.clone());
        ui.open_global().await?;

        let all = select_chats(&fx, p.chats + p.warm_chats)?;
        let n_targets = p.chats;
        for chat in &all {
            ui.open_chat(&chat.id, &fx.space_path(chat)).await?;
            ui.watch(&chat.id)?
                .wait_for(Duration::from_secs(90), "the full reset", |s| {
                    s.full.is_some()
                })
                .await?;
        }
        let script_dir = ctx.trial.root.join("scripts");
        fs::create_dir_all(&script_dir)?;
        let script = script_dir.join("pong.zbr.jsonl");
        fs::write(&script, pong_script())?;
        let cover = script_dir.join("warm-cover.zbr.jsonl");
        fs::write(&cover, cover_script())?;

        let mut jobs = Vec::new();
        for chat in &all {
            jobs.push((
                chat.id.clone(),
                fx.space_path(chat),
                fixture::message_uuid(chat.n, 7200),
                format!("zb:echo park-{}", chat.chat8),
            ));
        }
        queue_all(&ui.client, jobs).await?;
        for chat in &all {
            let needle = format!("park-{}", chat.chat8);
            ui.watch(&chat.id)?
                .wait_for(Duration::from_secs(90), "the parking turn", move |s| {
                    turn_settled(&s.view) && last_turn_text(&s.view) == needle
                })
                .await?;
        }
        let park_at = Instant::now();

        let mut next_warm = Instant::now();
        let mut warm_round: u64 = 0;
        let mut saw_all_parents = false;
        let mut best_tree: u64 = 0;
        let mut best_family: u64 = 0;
        let parked_tree: u64;
        let parked_swap: u64;
        let mut ballast_ready = false;
        let mut last_pids: Vec<Option<u32>> = vec![None; all.len()];
        let ballast_deadline = park_at + Duration::from_millis(p.ballast_wait_ms);
        loop {
            warm_round =
                keepalive(&ui, &fx, &all[n_targets..], &mut next_warm, warm_round, p).await?;
            let census = census(ctx.engine_pid, &all);
            if census.tree_pss_kib > best_tree {
                best_tree = census.tree_pss_kib;
            }
            let family = census.family_kib();
            if family > best_family {
                best_family = family;
            }
            if census.hits.iter().all(|h| h.pid.is_some()) {
                saw_all_parents = true;
            }
            for (slot, hit) in last_pids.iter_mut().zip(&census.hits) {
                if hit.pid.is_some() {
                    *slot = hit.pid;
                }
            }
            let children_ready = census
                .hits
                .iter()
                .all(|h| h.pid.is_some() && h.children >= n_child as usize);
            // Children appear only after the parent has touched its ballast,
            // but the children are still touching theirs. Freeze once the
            // whole fleet is actually resident (PSS or SwapPss).
            if children_ready && family >= ballast_floor_kib(all.len() as u64, per_mib) {
                ballast_ready = true;
                parked_tree = census.tree_pss_kib;
                parked_swap = census.tree_swap_pss_kib;
                break;
            }
            let targets_gone = census.hits[..n_targets]
                .iter()
                .zip(&last_pids[..n_targets])
                .any(|(hit, seen)| seen.is_some() && hit.pid.is_none());
            if targets_gone || Instant::now() >= ballast_deadline {
                parked_tree = best_tree;
                parked_swap = census.tree_swap_pss_kib;
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        ctx.marks.insert("parked".into(), mono_now());
        let pss_parked = kib_to_mib(parked_tree);
        ctx.metric("pss_parked", pss_parked);
        ctx.metric("swap_pss_parked", kib_to_mib(parked_swap));
        ctx.metric("replay_pss_parked", kib_to_mib(best_family));

        ctx.open_window().await?;
        let t_window = mono_now();

        let warm_busy = prime_warm(
            &ui,
            &fx,
            &all[n_targets..],
            &mut next_warm,
            &mut warm_round,
            p,
            &cover,
        )
        .await?;
        let rpc = request_hibernate(&ui.client, &all[..n_targets], warm_busy).await;

        let idle_deadline = park_at + Duration::from_millis(p.idle_ms.saturating_add(p.grace_ms));
        loop {
            warm_round =
                keepalive(&ui, &fx, &all[n_targets..], &mut next_warm, warm_round, p).await?;
            let alive = targets_alive(&last_pids[..n_targets]);
            if alive == 0 || Instant::now() >= idle_deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        let after_idle = census(ctx.engine_pid, &all);
        let ended = (0..n_targets)
            .filter(|&i| match (last_pids[i], after_idle.hits[i].pid) {
                (Some(before), Some(after)) => before != after,
                (Some(_), None) => true,
                _ => false,
            })
            .count();
        ctx.marks.insert("hibernated".into(), mono_now());
        let pss_hibernated = kib_to_mib(after_idle.tree_pss_kib);
        ctx.metric("pss_hibernated", pss_hibernated);
        ctx.metric(
            "swap_pss_hibernated",
            kib_to_mib(after_idle.tree_swap_pss_kib),
        );
        ctx.metric("pss_savings", pss_parked - pss_hibernated);
        ctx.metric("replay_pss_hibernated", kib_to_mib(after_idle.family_kib()));

        let rpc_fired = rpc.idle_n.unwrap_or(0) + rpc.chat_n > 0;
        let (outcome, outcome_detail) = if ended == 0 {
            (
                OUTCOME_STAYED,
                format!(
                    "stayed parked ({}; reaper did not end the {} target sessions within {} ms)",
                    rpc.detail.trim(),
                    n_targets,
                    p.idle_ms + p.grace_ms
                ),
            )
        } else if rpc_fired {
            (
                OUTCOME_HIBERNATED,
                format!(
                    "hibernated via RPC, {} target processes ended, PSS {pss_parked:.0} → {pss_hibernated:.0} MiB ({})",
                    ended,
                    rpc.detail.trim()
                ),
            )
        } else {
            (
                OUTCOME_REAPED,
                format!(
                    "reaped {ended} target sessions by the idle reaper ({})",
                    rpc.detail.trim()
                ),
            )
        };
        ctx.metric("outcome", outcome);
        ctx.metric(
            "reaped_sessions",
            if outcome == OUTCOME_REAPED {
                ended as f64
            } else {
                0.0
            },
        );
        ctx.metric(
            "hibernated_sessions",
            if outcome == OUTCOME_HIBERNATED {
                ended as f64
            } else {
                0.0
            },
        );
        ctx.metric(
            "stayed_sessions",
            if outcome == OUTCOME_STAYED {
                n_targets as f64
            } else {
                0.0
            },
        );
        eprintln!("zeron-bench: S-hibernate {outcome_detail}");

        let warm_alive = after_idle.hits[n_targets..]
            .iter()
            .filter(|hit| hit.pid.is_some())
            .count();

        // A keep-alive still in flight would delay the measured turn and reset
        // nothing we still need. Wait it out, then sample state before send.
        wait_settled(&ui, &all[n_targets..], Duration::from_secs(8)).await?;
        let state_dir = ctx.trial.root.join("replay-state");
        let prior_states = load_states(&state_dir);
        let priors: Vec<Option<ReplayState>> = all
            .iter()
            .map(|chat| state_for_chat(&prior_states, &chat.chat8).cloned())
            .collect();
        let before_events = ctx.probe.snapshot().len();
        let entries_before: Vec<usize> = all
            .iter()
            .map(|chat| ui.watch(&chat.id).map(|w| w.with(|s| s.view.entries.len())))
            .collect::<Result<_>>()?;
        let markers_before: Vec<usize> = all
            .iter()
            .map(|chat| ui.watch(&chat.id).map(|w| w.with(|s| s.markers.len())))
            .collect::<Result<_>>()?;
        for chat in &all {
            ui.watch(&chat.id)?.arm();
        }
        let mut jobs = Vec::new();
        for chat in &all {
            jobs.push((
                chat.id.clone(),
                fx.space_path(chat),
                fixture::message_uuid(chat.n, 7400),
                format!("zb:play {} speed=0 markers=1", script.display()),
            ));
        }
        let sends = queue_timed(&ui.client, jobs).await?;
        for chat in &all {
            let marker = format!("[[zb:{}:", chat.chat8);
            ui.watch(&chat.id)?
                .wait_for(
                    Duration::from_millis(p.resume_wait_ms),
                    "the measured turn",
                    move |s| {
                        if bad_status(&s.view).is_some() || last_turn_has_error_part(&s.view) {
                            return true;
                        }
                        turn_settled(&s.view) && last_turn_text(&s.view).contains(&marker)
                    },
                )
                .await?;
        }

        let after_states = load_states(&state_dir);
        let proc_after = census(ctx.engine_pid, &all);
        let events = ctx.probe.snapshot();
        let new_events = &events[before_events.min(events.len())..];

        let mut resume_ms = Vec::new();
        let mut resume_marker_ms = Vec::new();
        let mut warm_ms = Vec::new();
        let mut warm_marker_ms = Vec::new();
        let mut spawn_ms = Vec::new();
        let mut init_ms = Vec::new();
        let mut classes = Vec::new();
        let mut resume_fail = Vec::new();
        let mut text_fail = Vec::new();
        let mut resumed_n = 0.0;
        let mut cold_n = 0.0;
        let mut stayed_n = 0.0;

        for (i, chat) in all.iter().enumerate() {
            let expected = measured_text(&chat.chat8);
            let (first, marker_t, entries, err, text) = ui.watch(&chat.id)?.with(|s| {
                (
                    s.after,
                    s.markers.get(markers_before[i]).map(|m| m.2),
                    s.view.entries.len(),
                    turn_error(&s.view),
                    last_turn_text(&s.view),
                )
            });
            let sample = first.map(|t| ns_ms(t, sends[i]));
            let marker_sample = marker_t.map(|t| ns_ms(t, sends[i]));
            if i < n_targets {
                if let Some(v) = sample {
                    resume_ms.push(v);
                }
                if let Some(v) = marker_sample {
                    resume_marker_ms.push(v);
                }
            } else {
                if let Some(v) = sample {
                    warm_ms.push(v);
                }
                if let Some(v) = marker_sample {
                    warm_marker_ms.push(v);
                }
            }
            let prior = priors[i].as_ref();
            let same = prior.and_then(|pstate| {
                after_states
                    .iter()
                    .find(|s| s.session_id == pstate.session_id)
                    .cloned()
            });
            let fresh = after_states
                .iter()
                .find(|s| {
                    prior.is_none_or(|pstate| s.session_id != pstate.session_id)
                        && s.transcript.contains(&expected)
                })
                .cloned();
            let init_same = prior.and_then(|pstate| {
                new_events.iter().find_map(|(_, ev)| match ev {
                    ProbeEvent::Init {
                        session,
                        resumed,
                        t,
                    } if session == &pstate.session_id && *t >= sends[i] => Some((*resumed, *t)),
                    _ => None,
                })
            });
            let init_fresh = fresh.as_ref().and_then(|f| {
                new_events.iter().find_map(|(_, ev)| match ev {
                    ProbeEvent::Init {
                        session,
                        resumed,
                        t,
                    } if session == &f.session_id && *t >= sends[i] => Some((*resumed, *t)),
                    _ => None,
                })
            });
            let new_init = if init_same.is_some_and(|(resumed, _)| resumed) {
                init_same
            } else {
                init_fresh.or(init_same)
            };
            if let Some((_, t)) = new_init {
                init_ms.push(ns_ms(t, sends[i]));
            }
            let after_pid = proc_after.hits[i].pid;
            let pid_before = after_idle.hits[i].pid;
            if let Some(pid) = after_pid
                && pid_before != Some(pid)
                && let Some(start) = proc_start_ns(pid)
            {
                let ms = ns_ms(start, sends[i]);
                // starttime and CLOCK_MONOTONIC share boot only when the
                // machine has not suspended. Drop a sample that cannot be a
                // resume spawn rather than publishing a nonsense latency.
                if (0.0..120_000.0).contains(&ms) {
                    spawn_ms.push(ms);
                }
            }
            let pid_changed = match (pid_before, after_pid) {
                (Some(a), Some(b)) => a != b,
                (None, Some(_)) => true,
                (Some(_), None) => true,
                (None, None) => true,
            };
            let cmdline_resume = after_pid.and_then(|pid| resume_arg(&cmdline(pid)));
            let class = match prior {
                None => Err("no replay state for the parking turn".into()),
                Some(pstate) => classify(&ResumeInput {
                    prior_turns: pstate.turns,
                    prior_transcript: &pstate.transcript,
                    same: same.as_ref(),
                    fresh: fresh.as_ref(),
                    new_init_resumed: new_init.map(|(resumed, _)| resumed),
                    cmdline_resume: cmdline_resume.as_deref(),
                    prior_session: &pstate.session_id,
                    pid_changed,
                    error_text: err,
                    measured: &expected,
                }),
            };
            match &class {
                Ok(ResumeClass::Resumed) => resumed_n += 1.0,
                Ok(ResumeClass::ColdStart) => cold_n += 1.0,
                Ok(ResumeClass::StayedLive) => stayed_n += 1.0,
                Err(detail) => resume_fail.push(format!("{}: {detail}", chat.chat8)),
            }
            let growth = entries as i64 - entries_before[i] as i64;
            let text_ok = text.contains(&expected) && !err && growth <= 12;
            if !text_ok {
                text_fail.push(format!(
                    "{}: text_ok={} err={err} growth={growth} text={}",
                    chat.chat8,
                    text.contains(&expected),
                    clip(&text, 120)
                ));
            }
            let label = match &class {
                Ok(ResumeClass::Resumed) => "resumed with --resume and prior state".to_string(),
                Ok(ResumeClass::ColdStart) => "baseline: cold start, no resume".to_string(),
                Ok(ResumeClass::StayedLive) => {
                    "session stayed live; follow-up was a steer, not a --resume".to_string()
                }
                Err(_) => "fail".to_string(),
            };
            classes.push(format!("{}: {label}", chat.chat8));
        }

        ctx.metric("t_resume", percentile(&resume_ms, 50.0));
        ctx.metric("t_resume_max", percentile(&resume_ms, 100.0));
        ctx.metric("t_resume_marker", percentile(&resume_marker_ms, 50.0));
        ctx.metric("t_warm", percentile(&warm_ms, 50.0));
        ctx.metric("t_warm_max", percentile(&warm_ms, 100.0));
        ctx.metric("t_warm_marker", percentile(&warm_marker_ms, 50.0));
        if !spawn_ms.is_empty() {
            ctx.metric("t_spawn_resume", percentile(&spawn_ms, 50.0));
        }
        if !init_ms.is_empty() {
            ctx.metric("t_init_resume", percentile(&init_ms, 50.0));
        }
        let spawns = events
            .iter()
            .filter(|(_, ev)| match ev {
                ProbeEvent::Spawn { prog, t, .. } => prog == "zb-replay" && *t >= t_window,
                _ => false,
            })
            .count();
        ctx.metric("spawns", spawns as f64);
        ctx.metric("resumed_sessions", resumed_n);
        ctx.metric("cold_starts", cold_n);
        ctx.metric("stayed_live", stayed_n);

        let floor = ballast_floor_kib((n_targets + p.warm_chats) as u64, per_mib);
        ctx.check(
            "parked",
            saw_all_parents,
            if saw_all_parents {
                format!(
                    "{} sessions parked ({} targets + {} warm)",
                    all.len(),
                    n_targets,
                    p.warm_chats
                )
            } else {
                "a zb-replay parent never appeared".into()
            },
        );
        ctx.check(
            "ballast_resident",
            ballast_ready && best_family >= floor,
            format!(
                "replay PSS+SwapPss peak {best_family} KiB, floor {floor} KiB, children ready={ballast_ready}"
            ),
        );
        ctx.check("idle_outcome", true, outcome_detail);
        // Tree PSS is what the table reports. When pages sit in zram, a reaped
        // session drops SwapPss rather than PSS, so the ended case compares
        // the replay family's PSS+SwapPss (replay_pss_* metrics).
        let family_parked = kib_to_mib(best_family);
        let family_after = kib_to_mib(after_idle.family_kib());
        let consistent = if ended == 0 {
            pss_consistent(0, pss_parked, pss_hibernated, per_mib as f64)
        } else {
            pss_consistent(ended, family_parked, family_after, per_mib as f64)
        };
        ctx.check(
            "pss_consistent",
            consistent,
            format!(
                "ended={ended} pss {pss_parked:.1}→{pss_hibernated:.1} MiB, replay family {family_parked:.1}→{family_after:.1} MiB, per_session={per_mib} MiB"
            ),
        );
        ctx.check(
            "warm_kept",
            warm_alive == p.warm_chats,
            format!(
                "{warm_alive}/{} warm sessions still alive before the measured send",
                p.warm_chats
            ),
        );
        ctx.check(
            "resume_carries_state",
            resume_fail.is_empty(),
            if resume_fail.is_empty() {
                classes.join(", ")
            } else {
                resume_fail.join("; ")
            },
        );
        ctx.check(
            "transcript_continuous",
            text_fail.is_empty(),
            if text_fail.is_empty() {
                format!("each measured turn appended once ({})", classes.join(", "))
            } else {
                text_fail.join("; ")
            },
        );
        ctx.check(
            "samples",
            resume_ms.len() == n_targets && warm_ms.len() == p.warm_chats,
            format!(
                "t_resume samples {}/{n_targets}, t_warm samples {}/{}",
                resume_ms.len(),
                warm_ms.len(),
                p.warm_chats
            ),
        );

        ctx.close_window().await?;
        drop(ui);
        Ok(())
    }
}

fn select_chats(fx: &Fixture, n: usize) -> Result<Vec<ChatInfo>> {
    let chats: Vec<_> = fx
        .class("100k", true)
        .into_iter()
        .chain(fx.class("1m", true))
        .take(n)
        .collect();
    ensure!(
        chats.len() == n,
        "fixture has {} plain chats in 100k+1m, need {n}",
        chats.len()
    );
    Ok(chats)
}

async fn queue_all(client: &Client, jobs: Vec<(String, PathBuf, String, String)>) -> Result<()> {
    let _ = queue_timed(client, jobs).await?;
    Ok(())
}

/// Queue every prompt, returning the monotonic time immediately before each RPC.
async fn queue_timed(
    client: &Client,
    jobs: Vec<(String, PathBuf, String, String)>,
) -> Result<Vec<u64>> {
    let futs = jobs.into_iter().map(|(id, cwd, msg, prompt)| {
        let client = client.clone();
        async move {
            let t = mono_now();
            fixture::queue_prompt(&client, &id, &cwd, &msg, &prompt).await?;
            Ok::<_, anyhow::Error>(t)
        }
    });
    let mut times = Vec::with_capacity(futs.len());
    for result in futures::future::join_all(futs).await {
        times.push(result?);
    }
    Ok(times)
}

/// Queue a one-line echo on warm chats that have finished their previous turn.
/// Returns the next round number. The idle window restarts when the turn ends,
/// so the gap between keep-alives stays under `warm_every_ms`.
async fn keepalive(
    ui: &Ui,
    fx: &Fixture,
    warm: &[ChatInfo],
    next_warm: &mut Instant,
    round: u64,
    p: &HibernateParams,
) -> Result<u64> {
    if Instant::now() < *next_warm {
        return Ok(round);
    }
    let mut jobs = Vec::new();
    for chat in warm {
        let settled = ui
            .watch(&chat.id)?
            .with(|s| turn_settled(&s.view) && bad_status(&s.view).is_none());
        if !settled {
            continue;
        }
        jobs.push((
            chat.id.clone(),
            fx.space_path(chat),
            fixture::message_uuid(chat.n, 8000 + round as usize),
            format!("zb:echo w{round}"),
        ));
    }
    if jobs.is_empty() {
        return Ok(round);
    }
    queue_all(&ui.client, jobs).await?;
    *next_warm = Instant::now() + Duration::from_millis(p.warm_every_ms);
    Ok(round + 1)
}

/// Queue a slow cover turn on every warm chat and report whether each one is
/// mid-turn. `HibernateIdle` selects every idle chat, so the control must be
/// inside a turn before that RPC runs. An echo finishes before the poll can
/// see it; the cover holds `content_block_stop` for 1.5s after the text.
async fn prime_warm(
    ui: &Ui,
    fx: &Fixture,
    warm: &[ChatInfo],
    next_warm: &mut Instant,
    round: &mut u64,
    p: &HibernateParams,
    cover: &Path,
) -> Result<bool> {
    let settle_by = Instant::now() + Duration::from_secs(8);
    loop {
        let ready = warm.iter().all(|chat| {
            ui.watch(&chat.id)
                .map(|watch| watch.with(|s| turn_settled(&s.view) && bad_status(&s.view).is_none()))
                .unwrap_or(false)
        });
        if ready || Instant::now() >= settle_by {
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    let prompt = format!("zb:play {} speed=1 markers=0", cover.display());
    let jobs: Vec<_> = warm
        .iter()
        .map(|chat| {
            (
                chat.id.clone(),
                fx.space_path(chat),
                fixture::message_uuid(chat.n, 7300),
                prompt.clone(),
            )
        })
        .collect();
    if jobs.is_empty() {
        return Ok(false);
    }
    queue_all(&ui.client, jobs).await?;
    // The cover restarts the idle window when it ends. Do not stack an echo on it.
    *next_warm = Instant::now() + Duration::from_millis(p.warm_every_ms);
    *round += 1;
    let deadline = Instant::now() + Duration::from_millis(1200);
    loop {
        if warm_in_turn(ui, warm)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

fn warm_in_turn(ui: &Ui, warm: &[ChatInfo]) -> Result<bool> {
    if warm.is_empty() {
        return Ok(false);
    }
    for chat in warm {
        let in_turn = ui.watch(&chat.id)?.with(|s| !turn_settled(&s.view));
        if !in_turn {
            return Ok(false);
        }
    }
    Ok(true)
}

struct HibernateRpc {
    idle_n: Option<u64>,
    chat_n: u64,
    detail: String,
}

async fn request_hibernate(client: &Client, targets: &[ChatInfo], warm_busy: bool) -> HibernateRpc {
    let mut detail = String::new();
    let mut idle_n = None;
    let mut chat_n = 0u64;
    if warm_busy {
        match client.call("HibernateIdle", json!({})).await {
            Ok((value, _)) => {
                idle_n = Some(flag_count(&value));
                detail.push_str(&format!("HibernateIdle {} ", clip(&value.to_string(), 160)));
            }
            Err(err) => {
                detail.push_str(&format!(
                    "HibernateIdle error: {} ",
                    clip(&format!("{err:#}"), 200)
                ));
            }
        }
    } else {
        detail.push_str("HibernateIdle skipped (warm chats were not mid-turn) ");
    }
    for chat in targets {
        match client
            .call("HibernateChat", json!({"chatId": chat.id}))
            .await
        {
            Ok((value, _)) => chat_n += flag_count(&value),
            Err(err) => {
                detail.push_str(&format!(
                    "HibernateChat error: {} ",
                    clip(&format!("{err:#}"), 200)
                ));
                break;
            }
        }
    }
    if !detail.contains("HibernateChat error") {
        detail.push_str(&format!("HibernateChat hibernating={chat_n} "));
    }
    HibernateRpc {
        idle_n,
        chat_n,
        detail,
    }
}

fn flag_count(value: &Value) -> u64 {
    match value.get("hibernating") {
        Some(Value::Bool(true)) => 1,
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

async fn wait_settled(ui: &Ui, chats: &[ChatInfo], timeout: Duration) -> Result<()> {
    for chat in chats {
        ui.watch(&chat.id)?
            .wait_for(timeout, "the warm turn to settle", |s| {
                turn_settled(&s.view)
            })
            .await?;
    }
    Ok(())
}

fn targets_alive(pids: &[Option<u32>]) -> usize {
    pids.iter().filter(|pid| pid.is_some_and(pid_alive)).count()
}

fn turn_error(view: &ChatView) -> bool {
    last_turn_has_error_part(view)
        || bad_status(view).is_some()
        || last_turn_text(view).contains("No conversation found")
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct ReplayState {
    session_id: String,
    turns: u64,
    #[serde(default)]
    transcript: String,
}

fn load_states(dir: &Path) -> Vec<ReplayState> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return out;
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        if let Ok(state) = serde_json::from_slice::<ReplayState>(&bytes) {
            out.push(state);
        }
    }
    out
}

fn state_for_chat<'a>(states: &'a [ReplayState], chat8: &str) -> Option<&'a ReplayState> {
    let needle = format!("park-{chat8}");
    states
        .iter()
        .filter(|s| s.transcript.contains(&needle))
        .max_by_key(|s| s.turns)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResumeClass {
    Resumed,
    ColdStart,
    StayedLive,
}

struct ResumeInput<'a> {
    prior_session: &'a str,
    prior_turns: u64,
    prior_transcript: &'a str,
    same: Option<&'a ReplayState>,
    fresh: Option<&'a ReplayState>,
    /// `Some` when a new init frame names this chat's session. `None` when
    /// the process never restarted.
    new_init_resumed: Option<bool>,
    cmdline_resume: Option<&'a str>,
    pid_changed: bool,
    error_text: bool,
    measured: &'a str,
}

fn classify(inp: &ResumeInput<'_>) -> Result<ResumeClass, String> {
    if inp.error_text {
        return Err(
            "error result (No conversation found, or the turn failed); --resume without state fails the trial"
                .into(),
        );
    }
    let extended = |state: &ReplayState| -> bool {
        state.turns == inp.prior_turns + 1
            && state.transcript.starts_with(inp.prior_transcript)
            && state.transcript.ends_with(inp.measured)
            && state.transcript.matches(inp.measured).count() == 1
            && !state.transcript[inp.prior_transcript.len()..].contains(inp.prior_transcript)
    };
    if inp.new_init_resumed == Some(true) {
        let Some(same) = inp.same else {
            return Err("resumed init but the state file is gone".into());
        };
        if same.session_id != inp.prior_session {
            return Err("resumed init session id does not match the parked state".into());
        }
        if inp.cmdline_resume != Some(inp.prior_session) {
            return Err(format!(
                "init resumed but cmdline --resume={:?} != {}",
                inp.cmdline_resume, inp.prior_session
            ));
        }
        if !extended(same) {
            return Err(format!(
                "resume did not extend state by one turn (turns {} -> {})",
                inp.prior_turns, same.turns
            ));
        }
        return Ok(ResumeClass::Resumed);
    }
    if inp.new_init_resumed == Some(false) {
        if let Some(same) = inp.same
            && (same.turns != inp.prior_turns || same.transcript != inp.prior_transcript)
        {
            return Err("cold start modified the previous state file".into());
        }
        if inp.cmdline_resume.is_some() {
            return Err(format!(
                "cold start still passed --resume={:?}",
                inp.cmdline_resume
            ));
        }
        let Some(fresh) = inp.fresh else {
            return Err("baseline: cold start produced no new replay state".into());
        };
        if fresh.turns < 1 || fresh.transcript.matches(inp.measured).count() != 1 {
            return Err("cold start state is not the measured turn".into());
        }
        return Ok(ResumeClass::ColdStart);
    }
    if !inp.pid_changed {
        let Some(same) = inp.same else {
            return Err("live session has no replay state".into());
        };
        if !extended(same) {
            return Err(format!(
                "live steer did not extend state by one turn (turns {} -> {})",
                inp.prior_turns, same.turns
            ));
        }
        return Ok(ResumeClass::StayedLive);
    }
    Err("pid changed without an init frame; resume was neither confirmed nor a cold start".into())
}

fn pss_consistent(
    ended: usize,
    parked_mib: f64,
    hibernated_mib: f64,
    per_session_mib: f64,
) -> bool {
    if ended == 0 {
        parked_mib > 1.0 && (hibernated_mib - parked_mib).abs() / parked_mib < 0.25
    } else {
        let savings = parked_mib - hibernated_mib;
        savings >= 0.7 * ended as f64 * per_session_mib
    }
}

fn ballast_floor_kib(sessions: u64, per_session_mib: u64) -> u64 {
    sessions
        .saturating_mul(per_session_mib)
        .saturating_mul(1024)
        * 90
        / 100
}

struct ProcSnap {
    pid: u32,
    ppid: u32,
    cmd: Vec<String>,
    pss_kib: u64,
    swap_pss_kib: u64,
}

struct ChatHit {
    pid: Option<u32>,
    children: usize,
    family_kib: u64,
}

struct Census {
    tree_pss_kib: u64,
    tree_swap_pss_kib: u64,
    hits: Vec<ChatHit>,
}

impl Census {
    fn family_kib(&self) -> u64 {
        self.hits.iter().map(|h| h.family_kib).sum()
    }
}

fn census(engine_pid: u32, chats: &[ChatInfo]) -> Census {
    let edges = list_ppids();
    let tree = tree_pids(engine_pid, &edges);
    let mut procs = Vec::with_capacity(tree.len());
    let mut tree_pss_kib = 0u64;
    let mut tree_swap_pss_kib = 0u64;
    for pid in tree {
        let smaps = fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
            .map(|text| parse_smaps_rollup(&text))
            .unwrap_or_default();
        tree_pss_kib += smaps.pss_kib;
        tree_swap_pss_kib += smaps.swap_pss_kib;
        procs.push(ProcSnap {
            pid,
            ppid: edges.get(&pid).copied().unwrap_or(0),
            cmd: cmdline(pid),
            pss_kib: smaps.pss_kib,
            swap_pss_kib: smaps.swap_pss_kib,
        });
    }
    let hits = chats
        .iter()
        .map(|chat| hit_for_chat(&procs, &chat.id))
        .collect();
    Census {
        tree_pss_kib,
        tree_swap_pss_kib,
        hits,
    }
}

fn hit_for_chat(procs: &[ProcSnap], chat_id: &str) -> ChatHit {
    let parents: Vec<&ProcSnap> = procs
        .iter()
        .filter(|p| is_replay_parent(&p.cmd) && p.cmd.iter().any(|a| a.contains(chat_id)))
        .collect();
    let Some(parent) = parents.into_iter().max_by_key(|p| p.pid) else {
        return ChatHit {
            pid: None,
            children: 0,
            family_kib: 0,
        };
    };
    let mut children = 0usize;
    let mut family = parent.pss_kib + parent.swap_pss_kib;
    for proc in procs {
        if proc.ppid == parent.pid && is_ballast_child(&proc.cmd) {
            children += 1;
            family += proc.pss_kib + proc.swap_pss_kib;
        }
    }
    ChatHit {
        pid: Some(parent.pid),
        children,
        family_kib: family,
    }
}

fn list_ppids() -> BTreeMap<u32, u32> {
    let mut out = BTreeMap::new();
    let Ok(rd) = fs::read_dir("/proc") else {
        return out;
    };
    for ent in rd.flatten() {
        let Some(pid) = ent.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        if let Some(ppid) = ppid_from_stat(&stat) {
            out.insert(pid, ppid);
        }
    }
    out
}

fn tree_pids(root: u32, edges: &BTreeMap<u32, u32>) -> Vec<u32> {
    let mut kids: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (pid, ppid) in edges {
        kids.entry(*ppid).or_default().push(*pid);
    }
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        if let Some(children) = kids.get(&out[i]) {
            out.extend(children);
        }
        i += 1;
    }
    out
}

fn ppid_from_stat(stat: &str) -> Option<u32> {
    let rest = stat.rsplit_once(')')?.1;
    let mut fields = rest.split_whitespace();
    fields.next()?;
    fields.next()?.parse().ok()
}

fn starttime_ticks(stat: &str) -> Option<u64> {
    let rest = stat.rsplit_once(')')?.1;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // Field 22 (starttime) is index 19 after the comm.
    fields.get(19)?.parse().ok()
}

fn proc_start_ns(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let ticks = starttime_ticks(&stat)?;
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    Some(ticks.saturating_mul(1_000_000_000) / hz)
}

fn cmdline(pid: u32) -> Vec<String> {
    let Ok(bytes) = fs::read(format!("/proc/{pid}/cmdline")) else {
        return Vec::new();
    };
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

fn is_replay_parent(cmd: &[String]) -> bool {
    cmd.iter().any(|a| a.contains("zb-replay")) && !is_ballast_child(cmd)
}

fn is_ballast_child(cmd: &[String]) -> bool {
    cmd.iter().any(|a| a == "--ballast-child")
}

fn resume_arg(cmd: &[String]) -> Option<String> {
    for (i, arg) in cmd.iter().enumerate() {
        if let Some(rest) = arg.strip_prefix("--resume=") {
            return Some(rest.to_string());
        }
        if arg == "--resume" {
            return cmd.get(i + 1).cloned();
        }
    }
    None
}

fn pid_alive(pid: u32) -> bool {
    pid > 0 && Path::new(&format!("/proc/{pid}")).exists()
}

fn kib_to_mib(kib: u64) -> f64 {
    kib as f64 / 1024.0
}

fn ns_ms(t: u64, origin: u64) -> f64 {
    t.saturating_sub(origin) as f64 / 1e6
}

fn clip(text: &str, n: usize) -> String {
    let mut end = n.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn parse_children(spec: &str) -> Result<(u64, u64)> {
    let (n, mib) = spec.split_once(':').context("children must be n:MiB")?;
    let n: u64 = n.parse().context("children count")?;
    let mib: u64 = mib.parse().context("children MiB")?;
    ensure!((1..=64).contains(&n), "children count must be 1..=64");
    Ok((n, mib))
}

fn per_session_mib(ballast: u64, children: &str) -> Result<u64> {
    let (n, mib) = parse_children(children)?;
    Ok(ballast.saturating_add(n.saturating_mul(mib)))
}

fn measured_text(chat8: &str) -> String {
    format!("pong [[zb:{chat8}:1]]")
}

/// Holds the warm chats mid-turn while `HibernateIdle` runs. The stop frame's
/// `dt` is the hold. `markers=0` keeps the hash check, so the text is `warm`.
fn cover_script() -> String {
    let text = "warm";
    let sha = text_sha256(text);
    format!(
        concat!(
            r#"{{"zb":"meta","v":1,"name":"warm-cover","seed":9,"text_bytes":4,"text_sha256":"{sha}","frames":3}}"#,
            "\n",
            r#"{{"dt":0,"f":{{"type":"stream_event","event":{{"type":"content_block_start","index":0,"content_block":{{"type":"text","text":""}}}}}}}}"#,
            "\n",
            r#"{{"dt":0,"f":{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"warm"}}}}}}}}"#,
            "\n",
            r#"{{"dt":1500,"f":{{"type":"stream_event","event":{{"type":"content_block_stop","index":0}}}}}}"#,
            "\n",
            r#"{{"dt":0,"zb":"turn_end","usage":{{"input_tokens":0,"output_tokens":0}}}}"#,
            "\n",
        ),
        sha = sha
    )
}

/// One text delta, `pong`. `markers=1` on the play line injects the chat marker
/// and skips the meta hash, so every chat can share this file.
fn pong_script() -> String {
    let text = "pong";
    let sha = text_sha256(text);
    format!(
        concat!(
            r#"{{"zb":"meta","v":1,"name":"pong","seed":9,"text_bytes":4,"text_sha256":"{sha}","frames":3}}"#,
            "\n",
            r#"{{"dt":0,"f":{{"type":"stream_event","event":{{"type":"content_block_start","index":0,"content_block":{{"type":"text","text":""}}}}}}}}"#,
            "\n",
            r#"{{"dt":0,"f":{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"pong"}}}}}}}}"#,
            "\n",
            r#"{{"dt":0,"f":{{"type":"stream_event","event":{{"type":"content_block_stop","index":0}}}}}}"#,
            "\n",
            r#"{{"dt":0,"zb":"turn_end","usage":{{"input_tokens":0,"output_tokens":0}}}}"#,
            "\n",
        ),
        sha = sha
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_keeps_six_chats_and_the_idle_env() {
        let scenario = Hibernate::from_params(Tier::Quick, &[]).unwrap();
        assert_eq!(scenario.p.chats, 6);
        assert_eq!(scenario.p.warm_chats, 2);
        assert_eq!(scenario.p.idle_ms, 20_000);
        assert_eq!(scenario.p.ballast_mib, 300);
        assert_eq!(scenario.variant_key(), "ballast-6-quick");
        let env: BTreeMap<_, _> = scenario.engine_env().into_iter().collect();
        assert_eq!(env["ZB_REPLAY_MODE"], "ballast");
        assert_eq!(env["ZB_REPLAY_BALLAST_MIB"], "300");
        assert_eq!(env["ZB_REPLAY_CHILDREN"], "2:250");
        assert_eq!(env["ZB_REPLAY_STARTUP_MS"], "0");
        assert_eq!(env["ZB_REPLAY_RESUME_MS_PER_MB"], "0");
        assert_eq!(env["ZERON_SESSION_IDLE_MS"], "20000");
        assert!(Hibernate::from_params(Tier::Quick, &[("mode".into(), "real".into())]).is_err());
        assert!(
            Hibernate::from_params(Tier::Quick, &[("children".into(), "nope".into())]).is_err()
        );
    }

    #[test]
    fn children_and_floor_match_the_800_mib_session() {
        assert_eq!(parse_children("2:250").unwrap(), (2, 250));
        assert_eq!(per_session_mib(300, "2:250").unwrap(), 800);
        assert_eq!(ballast_floor_kib(8, 800), 8 * 800 * 1024 * 90 / 100);
    }

    #[test]
    fn pong_script_parses_and_the_marker_text_is_chat_specific() {
        let script = crate::replay::script::Script::parse(&pong_script()).unwrap();
        assert_eq!(script.meta.frames, 3);
        assert_eq!(script.meta.text_bytes, 4);
        assert_eq!(script.meta.text_sha256, text_sha256("pong"));
        assert_eq!(measured_text("0000000a"), "pong [[zb:0000000a:1]]");
        let cover = crate::replay::script::Script::parse(&cover_script()).unwrap();
        assert_eq!(cover.meta.frames, 3);
        assert_eq!(cover.meta.text_bytes, 4);
        assert_eq!(cover.meta.text_sha256, text_sha256("warm"));
        match &cover.steps[2] {
            crate::replay::script::Step::Frame { dt, .. } => assert_eq!(*dt, 1500),
            other => panic!("cover stop frame missing: {other:?}"),
        }
    }

    #[test]
    fn stat_and_resume_argv_parse() {
        let stat = "12 (zb replay) S 456 12 12 0 -1 0 0 0 0 0 1 2 3 4 20 0 1 0 99999 1 0";
        assert_eq!(ppid_from_stat(stat), Some(456));
        assert_eq!(starttime_ticks(stat), Some(99999));
        let cmd = vec![
            "zb-replay".into(),
            "--print".into(),
            "--resume=abc-1".into(),
        ];
        assert_eq!(resume_arg(&cmd).as_deref(), Some("abc-1"));
        let split = vec!["zb-replay".into(), "--resume".into(), "abc-2".into()];
        assert_eq!(resume_arg(&split).as_deref(), Some("abc-2"));
        assert!(is_replay_parent(&cmd));
        assert!(is_ballast_child(&[
            "zb-replay".into(),
            "--ballast-child".into(),
            "250".into()
        ]));
    }

    fn prior() -> ReplayState {
        ReplayState {
            session_id: "sess-1".into(),
            turns: 3,
            transcript: "park-0000000a".into(),
        }
    }

    #[test]
    fn resume_classification() {
        let base = prior();
        let extended = ReplayState {
            session_id: "sess-1".into(),
            turns: 4,
            transcript: format!("park-0000000a{}", measured_text("0000000a")),
        };
        let resumed = classify(&ResumeInput {
            prior_session: &base.session_id,
            prior_turns: base.turns,
            prior_transcript: &base.transcript,
            same: Some(&extended),
            fresh: None,
            new_init_resumed: Some(true),
            cmdline_resume: Some("sess-1"),
            pid_changed: true,
            error_text: false,
            measured: &measured_text("0000000a"),
        })
        .unwrap();
        assert_eq!(resumed, ResumeClass::Resumed);

        let fresh = ReplayState {
            session_id: "sess-new".into(),
            turns: 1,
            transcript: measured_text("0000000a"),
        };
        let cold = classify(&ResumeInput {
            prior_session: &base.session_id,
            prior_turns: base.turns,
            prior_transcript: &base.transcript,
            same: Some(&base),
            fresh: Some(&fresh),
            new_init_resumed: Some(false),
            cmdline_resume: None,
            pid_changed: true,
            error_text: false,
            measured: &measured_text("0000000a"),
        })
        .unwrap();
        assert_eq!(cold, ResumeClass::ColdStart);

        let stayed = classify(&ResumeInput {
            prior_session: &base.session_id,
            prior_turns: base.turns,
            prior_transcript: &base.transcript,
            same: Some(&extended),
            fresh: None,
            new_init_resumed: None,
            cmdline_resume: None,
            pid_changed: false,
            error_text: false,
            measured: &measured_text("0000000a"),
        })
        .unwrap();
        assert_eq!(stayed, ResumeClass::StayedLive);

        let missing = classify(&ResumeInput {
            prior_session: &base.session_id,
            prior_turns: base.turns,
            prior_transcript: &base.transcript,
            same: Some(&base),
            fresh: None,
            new_init_resumed: Some(true),
            cmdline_resume: Some("sess-1"),
            pid_changed: true,
            error_text: true,
            measured: &measured_text("0000000a"),
        });
        assert!(missing.is_err());
    }

    #[test]
    fn pss_movement_matches_the_outcome() {
        assert!(pss_consistent(0, 7000.0, 6900.0, 800.0));
        assert!(!pss_consistent(0, 7000.0, 4000.0, 800.0));
        assert!(pss_consistent(6, 7000.0, 7000.0 - 6.0 * 800.0, 800.0));
        assert!(!pss_consistent(6, 7000.0, 6800.0, 800.0));
    }
}
