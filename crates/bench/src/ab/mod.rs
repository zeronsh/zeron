//! A/B orchestrator: build revisions, alternate trials, score pairs, write the report.
//!
//! A trial's sampled numbers are [`crate::sample::Metrics`]. [`TrialMetrics`]
//! adds the fields the sampler does not fill (instructions, cycles, package
//! energy, stream latency, scenario extras) and flattens both into the dotted
//! ids statistics compare. [`FakeScenario`] replays injected values so the
//! protocol can be tested without an engine. S-build is cancelled and refused.

mod build;
mod report;
mod schedule;
mod stats;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use crate::sample::Metrics;

pub use build::{
    BuildRequest, BuiltBinary, build_revision, default_jobserver_fifo, default_target_parent,
    default_worktree_parent, distinct_copies,
};
pub use report::headline_metric_ids;
pub use schedule::{PlannedTrial, Side, Tier, abba, exceeds_spread, median};
pub use stats::{
    Fingerprint, MetricSummary, NoiseStatus, Verdict, lookup_noise, noise_pct_of,
    percentile_nearest_rank,
};

use stats::{noise_from_trials, store_noise, summarize_trials};

pub const TRIAL_SCHEMA: &str = "zeron-bench/trial/v1";

/// [`crate::sample::Metrics`] plus the §2.1 fields that sampler does not fill.
///
/// `instructions_u`, `cycles_u` and `energy_j_net` stay absent from JSON until
/// a trial records them. Stats read dotted ids from [`TrialMetrics::flatten`].
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TrialMetrics {
    #[serde(flatten)]
    pub sampled: Metrics,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_u: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycles_u: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy_j_net: Option<f64>,
    #[serde(default, skip_serializing_if = "LatencyMs::is_empty")]
    pub latency_ms: LatencyMs,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scenario: BTreeMap<String, f64>,
}

impl TrialMetrics {
    pub fn from_sampled(sampled: Metrics) -> Self {
        Self {
            sampled,
            ..Self::default()
        }
    }
}

impl Deref for TrialMetrics {
    type Target = Metrics;

    fn deref(&self) -> &Metrics {
        &self.sampled
    }
}

impl DerefMut for TrialMetrics {
    fn deref_mut(&mut self) -> &mut Metrics {
        &mut self.sampled
    }
}

impl From<Metrics> for TrialMetrics {
    fn from(sampled: Metrics) -> Self {
        Self::from_sampled(sampled)
    }
}

/// Deserialize fills missing sampler fields from [`Metrics::default`], so an
/// inject file can name only the numbers it cares about.
impl<'de> Deserialize<'de> for TrialMetrics {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        trial_metrics_from_value(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize)]
struct TrialMetricsDe {
    #[serde(flatten)]
    sampled: Metrics,
    #[serde(default)]
    instructions_u: Option<f64>,
    #[serde(default)]
    cycles_u: Option<f64>,
    #[serde(default)]
    energy_j_net: Option<f64>,
    #[serde(default)]
    latency_ms: LatencyMs,
    #[serde(default)]
    scenario: BTreeMap<String, f64>,
}

fn trial_metrics_from_value(value: Value) -> std::result::Result<TrialMetrics, String> {
    let mut base = serde_json::to_value(TrialMetrics::default()).map_err(|err| err.to_string())?;
    merge_json(&mut base, value);
    let parsed: TrialMetricsDe = serde_json::from_value(base).map_err(|err| err.to_string())?;
    Ok(TrialMetrics {
        sampled: parsed.sampled,
        instructions_u: parsed.instructions_u,
        cycles_u: parsed.cycles_u,
        energy_j_net: parsed.energy_j_net,
        latency_ms: parsed.latency_ms,
        scenario: parsed.scenario,
    })
}

fn merge_json(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Object(dst), Value::Object(src)) => {
            for (key, value) in src {
                if let Some(slot) = dst.get_mut(&key) {
                    merge_json(slot, value);
                } else {
                    dst.insert(key, value);
                }
            }
        }
        (slot, over) => *slot = over,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LatencyMs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<StreamLatency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpc_rtt: Option<RpcRtt>,
}

impl LatencyMs {
    fn is_empty(&self) -> bool {
        self.stream.is_none() && self.rpc_rtt.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StreamLatency {
    pub n: u64,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
    pub max: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcRtt {
    pub p50: f64,
    pub p99: f64,
}

/// Read the dotted ids statistics compare.
pub trait MetricView {
    fn get(&self, id: &str) -> Option<f64>;
    fn ids(&self) -> Vec<String>;
}

impl MetricView for TrialMetrics {
    fn get(&self, id: &str) -> Option<f64> {
        self.flatten().get(id).copied()
    }

    fn ids(&self) -> Vec<String> {
        self.flatten().into_keys().collect()
    }
}

impl TrialMetrics {
    /// Flat ids (`cpu_s.engine`, `wakeups_per_s.comm.<name>`, …).
    /// Optional fields that were not collected are absent, not zero.
    pub fn flatten(&self) -> BTreeMap<String, f64> {
        let mut out = BTreeMap::new();
        let sampled = &self.sampled;
        put_f64(&mut out, "cpu_s.tree", sampled.cpu_s.tree);
        put_f64(&mut out, "cpu_s.engine", sampled.cpu_s.engine);
        put_f64(&mut out, "cpu_s.user", sampled.cpu_s.user);
        put_f64(&mut out, "cpu_s.system", sampled.cpu_s.system);
        for (name, value) in &sampled.cpu_s.by_group {
            put_f64(&mut out, format!("cpu_s.group.{name}"), *value);
        }
        put_f64(&mut out, "cpu_pct_core.tree", sampled.cpu_pct_core.tree);
        put_f64(&mut out, "cpu_pct_core.engine", sampled.cpu_pct_core.engine);
        if let Some(value) = self.instructions_u {
            put_f64(&mut out, "instructions_u", value);
        }
        if let Some(value) = self.cycles_u {
            put_f64(&mut out, "cycles_u", value);
        }
        put_f64(&mut out, "pss_mib.peak", sampled.pss_mib.peak);
        put_f64(&mut out, "pss_mib.mean", sampled.pss_mib.mean);
        put_f64(&mut out, "pss_mib.end", sampled.pss_mib.end);
        for (name, value) in &sampled.pss_mib.by_group_peak {
            put_f64(&mut out, format!("pss_mib.group.{name}"), *value);
        }
        for (name, value) in &sampled.pss_mib.by_group_end {
            put_f64(&mut out, format!("pss_mib.group_end.{name}"), *value);
        }
        put_mem(&mut out, "pss_anon_mib", &sampled.pss_anon_mib);
        put_mem(&mut out, "pss_file_mib", &sampled.pss_file_mib);
        put_mem(&mut out, "swap_pss_mib", &sampled.swap_pss_mib);
        if let Some(value) = sampled.cg_mem_peak_mib {
            put_f64(&mut out, "cg_mem_peak_mib", value);
        }
        if let Some(anon) = &sampled.cg_anon_mib {
            put_f64(&mut out, "cg_anon_mib.peak", anon.peak);
            put_f64(&mut out, "cg_anon_mib.mean", anon.mean);
            put_f64(&mut out, "cg_anon_mib.end", anon.end);
        }
        put_f64(&mut out, "wakeups_per_s.tree", sampled.wakeups_per_s.tree);
        for (name, value) in &sampled.wakeups_per_s.by_comm {
            put_f64(&mut out, format!("wakeups_per_s.comm.{name}"), *value);
        }
        put_f64(&mut out, "ctx_per_s.vol", sampled.ctx_per_s.vol);
        put_f64(&mut out, "ctx_per_s.invol", sampled.ctx_per_s.invol);
        put_f64(&mut out, "write_bytes", sampled.write_bytes as f64);
        put_f64(&mut out, "wchar", sampled.wchar as f64);
        put_f64(&mut out, "syscw", sampled.syscw as f64);
        let growth = &sampled.data_dir_growth;
        put_f64(&mut out, "data_dir_growth.sqlite", growth.sqlite as f64);
        put_f64(&mut out, "data_dir_growth.wal", growth.wal as f64);
        put_f64(&mut out, "data_dir_growth.journals", growth.journals as f64);
        put_f64(&mut out, "data_dir_growth.logs", growth.logs as f64);
        put_f64(
            &mut out,
            "data_dir_growth.previews_json",
            growth.previews_json as f64,
        );
        put_f64(&mut out, "data_dir_growth.other", growth.other as f64);
        put_f64(&mut out, "threads.max", sampled.threads.max as f64);
        put_f64(&mut out, "threads.end", sampled.threads.end as f64);
        for (name, value) in &sampled.threads.by_comm {
            put_f64(&mut out, format!("threads.comm.{name}"), f64::from(*value));
        }
        put_f64(&mut out, "arena_heaps", f64::from(sampled.arena_heaps));
        put_f64(&mut out, "fds", f64::from(sampled.fds));
        put_f64(
            &mut out,
            "inotify.watches",
            f64::from(sampled.inotify.watches),
        );
        put_f64(&mut out, "inotify.dup", f64::from(sampled.inotify.dup));
        for (name, value) in &sampled.spawns {
            put_f64(&mut out, format!("spawns.{name}"), *value as f64);
        }
        if let Some(stream) = &self.latency_ms.stream {
            if stream.n > 0 {
                put_f64(&mut out, "latency_ms.stream.p50", stream.p50);
                put_f64(&mut out, "latency_ms.stream.p90", stream.p90);
                put_f64(&mut out, "latency_ms.stream.p99", stream.p99);
                put_f64(&mut out, "latency_ms.stream.max", stream.max);
            }
        }
        if let Some(rtt) = &self.latency_ms.rpc_rtt {
            put_f64(&mut out, "latency_ms.rpc_rtt.p50", rtt.p50);
            put_f64(&mut out, "latency_ms.rpc_rtt.p99", rtt.p99);
        }
        for (name, value) in &sampled.rpc.bytes_in {
            put_f64(&mut out, format!("rpc.bytes_in.{name}"), *value as f64);
        }
        for (name, value) in &sampled.rpc.items {
            put_f64(&mut out, format!("rpc.items.{name}"), *value as f64);
        }
        if let Some(energy) = self.energy_j_net {
            put_f64(&mut out, "energy_j_net", energy);
        }
        put_f64(&mut out, "outside_cpu_pct", sampled.outside_cpu_pct);
        if let Some(cpu) = sampled.real_engine_cpu_s {
            put_f64(&mut out, "real_engine_cpu_s", cpu);
        }
        if let Some(cpu) = sampled.real_engine_cpu_pct {
            put_f64(&mut out, "real_engine_cpu_pct", cpu);
        }
        if let Some(psi) = &sampled.psi_cpu {
            put_psi(&mut out, "psi_cpu", psi);
        }
        if let Some(psi) = &sampled.psi_mem {
            put_psi(&mut out, "psi_mem", psi);
        }
        for (name, value) in &self.scenario {
            put_f64(&mut out, format!("scenario.{name}"), *value);
        }
        out
    }
}

fn put_f64(out: &mut BTreeMap<String, f64>, key: impl Into<String>, value: f64) {
    if value.is_finite() {
        out.insert(key.into(), value);
    }
}

fn put_mem(out: &mut BTreeMap<String, f64>, prefix: &str, level: &crate::sample::MemLevel) {
    put_f64(out, format!("{prefix}.peak"), level.peak);
    put_f64(out, format!("{prefix}.mean"), level.mean);
    put_f64(out, format!("{prefix}.end"), level.end);
    for (name, value) in &level.by_group_peak {
        put_f64(out, format!("{prefix}.group.{name}"), *value);
    }
}

fn put_psi(out: &mut BTreeMap<String, f64>, prefix: &str, psi: &crate::sample::PsiWindow) {
    put_f64(out, format!("{prefix}.some_avg10"), psi.some_avg10);
    put_f64(out, format!("{prefix}.full_avg10"), psi.full_avg10);
    put_f64(
        out,
        format!("{prefix}.some_total_us"),
        psi.some_total_us as f64,
    );
    put_f64(
        out,
        format!("{prefix}.full_total_us"),
        psi.full_total_us as f64,
    );
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimesMs {
    pub spawn: u64,
    pub listen: u64,
    pub ready: u64,
    pub window_start: u64,
    pub window_end: u64,
    pub stopped: u64,
}

/// One finished trial. `pair_index == None` is a discarded warm-up.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrialRecord {
    pub schema: String,
    pub scenario: String,
    pub variant: Value,
    pub side: Side,
    pub rev: String,
    pub engine_sha256: String,
    pub fixture_id: String,
    pub seed: u64,
    pub order_index: u32,
    #[serde(default)]
    pub pair_index: Option<u32>,
    pub valid: bool,
    pub invalid_reason: Option<String>,
    pub noisy: bool,
    pub containment: String,
    pub times_ms: TimesMs,
    pub window_s: f64,
    pub metrics: TrialMetrics,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub counters_delta: BTreeMap<String, f64>,
    #[serde(default)]
    pub checks: Vec<Check>,
    /// Directory slug. Not part of the trial schema.
    #[serde(skip)]
    pub trial_dir: String,
    #[serde(skip)]
    pub variant_key: String,
}

impl TrialRecord {
    pub fn dir_name(&self) -> String {
        if !self.trial_dir.is_empty() {
            return self.trial_dir.clone();
        }
        let variant = if self.variant_key.is_empty() {
            "v"
        } else {
            self.variant_key.as_str()
        };
        format!(
            "{:03}-{}-{}-{}",
            self.order_index,
            self.side.as_str(),
            crate::env::sanitize(&self.scenario),
            crate::env::sanitize(variant)
        )
    }
}

/// What one scenario invocation returns. The protocol adds identity fields.
#[derive(Clone, Debug)]
pub struct TrialBody {
    pub metrics: TrialMetrics,
    pub counters_delta: BTreeMap<String, f64>,
    pub checks: Vec<Check>,
    pub valid: bool,
    pub invalid_reason: Option<String>,
    pub noisy: bool,
    pub containment: String,
    pub times_ms: TimesMs,
    pub window_s: f64,
}

impl TrialBody {
    pub fn ok(metrics: TrialMetrics) -> Self {
        Self {
            metrics,
            counters_delta: BTreeMap::new(),
            checks: vec![Check {
                name: "status_complete".into(),
                ok: true,
                detail: None,
            }],
            valid: true,
            invalid_reason: None,
            noisy: false,
            containment: "scope".into(),
            times_ms: TimesMs::default(),
            window_s: 1.0,
        }
    }
}

/// A scenario the orchestrator can run. Live scenarios land in a later unit.
pub trait Scenario {
    fn id(&self) -> &str;
    fn variant(&self) -> &Value;
    fn variant_key(&self) -> &str;
    fn fixture_id(&self) -> &str;
    fn run_trial(&mut self, planned: &PlannedTrial) -> Result<TrialBody>;
}

/// Injected trials, consumed per side in the order the protocol asks for them.
#[derive(Clone, Debug)]
pub struct FakeScenario {
    pub id: String,
    pub variant: Value,
    pub variant_key: String,
    pub fixture_id: String,
    queues: HashMap<Side, VecDeque<TrialBody>>,
    pub seen: Vec<PlannedTrial>,
}

impl FakeScenario {
    pub fn new(id: &str, variant_key: &str, fixture_id: &str) -> Self {
        Self {
            id: id.to_string(),
            variant: json!({"name": variant_key}),
            variant_key: variant_key.to_string(),
            fixture_id: fixture_id.to_string(),
            queues: HashMap::new(),
            seen: Vec::new(),
        }
    }

    pub fn push(&mut self, side: Side, body: TrialBody) {
        self.queues.entry(side).or_default().push_back(body);
    }
}

impl Scenario for FakeScenario {
    fn id(&self) -> &str {
        &self.id
    }
    fn variant(&self) -> &Value {
        &self.variant
    }
    fn variant_key(&self) -> &str {
        &self.variant_key
    }
    fn fixture_id(&self) -> &str {
        &self.fixture_id
    }
    fn run_trial(&mut self, planned: &PlannedTrial) -> Result<TrialBody> {
        self.seen.push(planned.clone());
        let queue = self
            .queues
            .get_mut(&planned.side)
            .with_context(|| format!("no injected trials for side {}", planned.side.as_str()))?;
        queue.pop_front().with_context(|| {
            format!(
                "injected trials for side {} ran out at order {}",
                planned.side.as_str(),
                planned.order_index
            )
        })
    }
}

#[derive(Clone, Debug)]
pub struct SideBuild {
    pub rev: String,
    pub sha256: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BinaryMeta {
    pub rev: String,
    pub sha256: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preflight {
    pub outside_cpu_pct: f64,
    pub real_engine_pid: Option<u32>,
    pub real_engine_cpu_pct: Option<f64>,
    pub rustc_outside: u32,
    pub quiet: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunMeta {
    pub label: String,
    pub tier: String,
    pub scenario: String,
    pub variant: Value,
    pub variant_key: String,
    pub fixture_id: String,
    pub fixture_manifest_sha: String,
    pub machine: String,
    pub kernel: String,
    pub governor: String,
    pub epp: String,
    pub amd_pstate: String,
    pub bios: String,
    pub loadavg: String,
    pub rustc: String,
    pub bench_git_sha: String,
    pub binaries: BTreeMap<String, BinaryMeta>,
    pub pairs_base: u32,
    pub pairs_run: u32,
    pub extra_pairs: u32,
    pub seed: u64,
    pub control: bool,
    pub noise: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight: Option<Preflight>,
}

pub struct ProtocolRequest {
    pub zb_root: PathBuf,
    pub run_id: String,
    pub label: String,
    pub tier: Tier,
    pub seed: u64,
    pub side_a: SideBuild,
    pub side_b: SideBuild,
    pub fingerprint: Fingerprint,
    pub now_unix: u64,
    pub bench_git_sha: String,
    pub rustc: String,
    pub loadavg: String,
    pub amd_pstate: String,
    pub control: bool,
    pub preflight: Option<Preflight>,
}

#[derive(Debug)]
pub struct ExecuteResult {
    pub run_dir: PathBuf,
    pub pairs_run: u32,
    pub extra_pairs: u32,
    pub report: String,
}

/// S-build was cancelled. Nothing in the bench runs it.
pub fn is_cancelled_scenario(name: &str) -> bool {
    let name = name.trim();
    name.eq_ignore_ascii_case("s-build") || name.eq_ignore_ascii_case("build")
}

/// Run the A/B (or A/A control) protocol for one scenario.
pub fn execute(req: &ProtocolRequest, scenario: &mut dyn Scenario) -> Result<ExecuteResult> {
    if is_cancelled_scenario(scenario.id()) {
        bail!("S-build is cancelled; the bench does not run it");
    }
    let run_dir = req
        .zb_root
        .join("runs")
        .join(crate::env::sanitize(&req.run_id));
    fs::create_dir_all(run_dir.join("trials"))
        .with_context(|| format!("create {}", run_dir.display()))?;
    let mut planned = Vec::new();
    schedule::push_warmup(&mut planned, req.seed);
    let mut records = Vec::new();
    run_planned(req, scenario, &run_dir, &planned, &mut records)?;
    let mut measured = 0u32;
    let mut target = req.tier.base_pairs();
    while measured < target {
        let start = planned.len();
        schedule::push_pair(&mut planned, measured, req.seed);
        run_planned(req, scenario, &run_dir, &planned[start..], &mut records)?;
        measured += 1;
        if measured >= req.tier.base_pairs()
            && measured == target
            && target < req.tier.max_pairs()
            && headline_spread(&records)
        {
            target += 1;
        }
    }
    let noise_dir = req.zb_root.join("noise");
    let status = if req.control {
        let map = noise_from_trials(&records);
        store_noise(
            &noise_dir,
            scenario.id(),
            scenario.variant_key(),
            &req.fingerprint,
            req.now_unix,
            &map,
        )?;
        NoiseStatus::Fresh(map)
    } else {
        lookup_noise(
            &noise_dir,
            scenario.id(),
            scenario.variant_key(),
            &req.fingerprint,
            req.now_unix,
        )
    };
    let noise_map = match &status {
        NoiseStatus::Fresh(map) => Some(map.clone()),
        NoiseStatus::Unknown(_) => None,
    };
    let stats = summarize_trials(&records, noise_map.as_ref());
    let note = status.note();
    let meta = run_meta(req, scenario, measured, &note);
    write_json(&run_dir.join("meta.json"), &meta)?;
    write_json(&run_dir.join("plan.json"), &plan_value(&meta, &planned))?;
    let report = report::write_summary_and_report(&run_dir, &meta, &stats, &records, &note)?;
    Ok(ExecuteResult {
        run_dir,
        pairs_run: measured,
        extra_pairs: measured.saturating_sub(req.tier.base_pairs()),
        report,
    })
}

fn run_planned(
    req: &ProtocolRequest,
    scenario: &mut dyn Scenario,
    run_dir: &Path,
    planned: &[PlannedTrial],
    records: &mut Vec<TrialRecord>,
) -> Result<()> {
    for slot in planned {
        let body = scenario.run_trial(slot)?;
        let side = match slot.side {
            Side::A => &req.side_a,
            Side::B => &req.side_b,
        };
        let mut record = TrialRecord {
            schema: TRIAL_SCHEMA.into(),
            scenario: scenario.id().to_string(),
            variant: scenario.variant().clone(),
            side: slot.side,
            rev: side.rev.clone(),
            engine_sha256: side.sha256.clone(),
            fixture_id: scenario.fixture_id().to_string(),
            seed: slot.seed,
            order_index: slot.order_index,
            pair_index: slot.pair_index,
            valid: body.valid,
            invalid_reason: body.invalid_reason,
            noisy: body.noisy,
            containment: body.containment,
            times_ms: body.times_ms,
            window_s: body.window_s,
            metrics: body.metrics,
            counters_delta: body.counters_delta,
            checks: body.checks,
            trial_dir: String::new(),
            variant_key: scenario.variant_key().to_string(),
        };
        record.trial_dir = record.dir_name();
        let dir = run_dir.join("trials").join(&record.trial_dir);
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        write_json(&dir.join("trial.json"), &record)?;
        records.push(record);
    }
    Ok(())
}

fn headline_spread(records: &[TrialRecord]) -> bool {
    headline_metric_ids().iter().any(|id| {
        [Side::A, Side::B].into_iter().any(|side| {
            let values: Vec<f64> = records
                .iter()
                .filter(|trial| trial.side == side && trial.pair_index.is_some())
                .filter_map(|trial| trial.metrics.get(id))
                .collect();
            exceeds_spread(&values)
        })
    })
}

fn run_meta(
    req: &ProtocolRequest,
    scenario: &dyn Scenario,
    pairs_run: u32,
    noise: &str,
) -> RunMeta {
    let mut binaries = BTreeMap::new();
    binaries.insert(
        "A".into(),
        BinaryMeta {
            rev: req.side_a.rev.clone(),
            sha256: req.side_a.sha256.clone(),
            path: req.side_a.path.display().to_string(),
        },
    );
    binaries.insert(
        "B".into(),
        BinaryMeta {
            rev: req.side_b.rev.clone(),
            sha256: req.side_b.sha256.clone(),
            path: req.side_b.path.display().to_string(),
        },
    );
    let fixture_manifest_sha = build::sha256_bytes(scenario.fixture_id().as_bytes());
    RunMeta {
        label: req.label.clone(),
        tier: req.tier.as_str().into(),
        scenario: scenario.id().into(),
        variant: scenario.variant().clone(),
        variant_key: scenario.variant_key().into(),
        fixture_id: scenario.fixture_id().into(),
        fixture_manifest_sha,
        machine: req.fingerprint.machine.clone(),
        kernel: req.fingerprint.kernel.clone(),
        governor: req.fingerprint.governor.clone(),
        epp: req.fingerprint.epp.clone(),
        amd_pstate: req.amd_pstate.clone(),
        bios: req.fingerprint.bios.clone(),
        loadavg: req.loadavg.clone(),
        rustc: req.rustc.clone(),
        bench_git_sha: req.bench_git_sha.clone(),
        binaries,
        pairs_base: req.tier.base_pairs(),
        pairs_run,
        extra_pairs: pairs_run.saturating_sub(req.tier.base_pairs()),
        seed: req.seed,
        control: req.control,
        noise: noise.into(),
        preflight: req.preflight.clone(),
    }
}

fn plan_value(meta: &RunMeta, planned: &[PlannedTrial]) -> Value {
    json!({
        "tier": meta.tier,
        "scenario": meta.scenario,
        "variant": meta.variant,
        "base_pairs": meta.pairs_base,
        "pairs_run": meta.pairs_run,
        "extra_pairs": meta.extra_pairs,
        "trials": planned.iter().map(|slot| json!({
            "order_index": slot.order_index,
            "side": slot.side.as_str(),
            "pair_index": slot.pair_index,
            "warmup": slot.pair_index.is_none(),
            "seed": slot.seed,
        })).collect::<Vec<_>>(),
    })
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let body = serde_json::to_vec_pretty(value).context("encode json")?;
    fs::write(path, body).with_context(|| format!("write {}", path.display()))
}

/// Rebuild `summary.json` and `report.md` from the trials already on disk.
pub fn regenerate_report(run_dir: &Path) -> Result<String> {
    let meta: RunMeta = read_json(&run_dir.join("meta.json"))?;
    let mut trials = load_trials(run_dir)?;
    for trial in &mut trials {
        if trial.variant_key.is_empty() {
            trial.variant_key = meta.variant_key.clone();
        }
    }
    let zb_root = run_dir
        .parent()
        .and_then(|runs| runs.parent())
        .context("run dir is not zb_root/runs/<id>")?;
    let fingerprint = Fingerprint {
        machine: meta.machine.clone(),
        kernel: meta.kernel.clone(),
        governor: meta.governor.clone(),
        epp: meta.epp.clone(),
        bios: meta.bios.clone(),
    };
    let status = lookup_noise(
        &zb_root.join("noise"),
        &meta.scenario,
        &meta.variant_key,
        &fingerprint,
        stats::now_unix(),
    );
    let noise_map = match &status {
        NoiseStatus::Fresh(map) => Some(map.clone()),
        NoiseStatus::Unknown(_) => None,
    };
    let summed = summarize_trials(&trials, noise_map.as_ref());
    report::write_summary_and_report(run_dir, &meta, &summed, &trials, &status.note())
}

fn load_trials(run_dir: &Path) -> Result<Vec<TrialRecord>> {
    let trials_dir = run_dir.join("trials");
    let mut dirs = Vec::new();
    for ent in
        fs::read_dir(&trials_dir).with_context(|| format!("read {}", trials_dir.display()))?
    {
        let ent = ent?;
        if ent.path().is_dir() {
            dirs.push(ent.path());
        }
    }
    dirs.sort();
    let mut trials = Vec::new();
    for dir in dirs {
        let mut trial: TrialRecord = read_json(&dir.join("trial.json"))?;
        trial.trial_dir = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        trials.push(trial);
    }
    Ok(trials)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

/// CLI-shaped options for `ab` and `control`.
pub struct AbOptions {
    pub a: String,
    pub b: String,
    pub scenario: String,
    pub tier: Tier,
    pub zb_root: PathBuf,
    pub repo: PathBuf,
    pub seed: u64,
    pub inject: Option<PathBuf>,
    pub no_build: bool,
    pub control: bool,
    pub preflight: bool,
    pub run_id: Option<String>,
    pub fingerprint: Option<Fingerprint>,
    pub now_unix: Option<u64>,
}

pub fn run_ab(opts: &AbOptions) -> Result<ExecuteResult> {
    if is_cancelled_scenario(&opts.scenario) {
        bail!("S-build is cancelled; the bench does not run it");
    }
    let fingerprint = opts.fingerprint.clone().unwrap_or_else(host_fingerprint);
    let now_unix = opts.now_unix.unwrap_or_else(stats::now_unix);
    let preflight = if opts.preflight {
        Some(collect_preflight())
    } else {
        None
    };
    let governor_before = preflight.as_ref().map(|_| governor_signature());
    let (side_a, side_b) = resolve_sides(opts)?;
    let run_id = opts.run_id.clone().unwrap_or_else(|| {
        let kind = if opts.control { "control" } else { "ab" };
        format!(
            "{}-{}-{}",
            crate::env::utc_compact(now_unix),
            kind,
            crate::env::sanitize(&opts.scenario)
        )
    });
    let label = if opts.control {
        format!("control {}", opts.scenario)
    } else {
        format!("ab {}", opts.scenario)
    };
    let mut scenario: Box<dyn Scenario> = match &opts.inject {
        Some(inject) => Box::new(load_inject(&opts.scenario, inject)?),
        None => live_scenario(opts, &run_id, &side_a, &side_b)?,
    };
    let req = ProtocolRequest {
        zb_root: opts.zb_root.clone(),
        run_id,
        label,
        tier: opts.tier,
        seed: opts.seed,
        side_a,
        side_b,
        fingerprint,
        now_unix,
        bench_git_sha: git_head(&opts.repo).unwrap_or_else(|_| "unknown".into()),
        rustc: read_trim_cmd("rustc"),
        loadavg: read_trim("/proc/loadavg").unwrap_or_else(|| "unknown".into()),
        amd_pstate: read_trim("/sys/devices/system/cpu/amd_pstate/status")
            .unwrap_or_else(|| "unknown".into()),
        control: opts.control,
        preflight,
    };
    let result = execute(&req, scenario.as_mut())?;
    if let Some(before) = governor_before {
        let after = governor_signature();
        if before != after {
            bail!(
                "governor or EPP changed during the run ({before} -> {after}); the verdict is void"
            );
        }
    }
    Ok(result)
}

/// A real scenario (S-stream, S-boot, S-idle, S-switch, S-hibernate) over the two built engines.
fn live_scenario(
    opts: &AbOptions,
    run_id: &str,
    side_a: &SideBuild,
    side_b: &SideBuild,
) -> Result<Box<dyn Scenario>> {
    if opts.no_build {
        bail!("live scenarios need built engines; drop --no-build or pass --inject");
    }
    let env = crate::scenario::RunEnv::new(
        opts.zb_root.clone(),
        crate::env::sanitize(run_id),
        opts.tier,
        opts.seed,
        side_a.path.clone(),
        side_b.path.clone(),
        std::env::current_exe().context("resolve zeron-bench")?,
        side_a.rev.clone(),
    )?;
    match crate::scenario::build(&opts.scenario, env)? {
        Some(scenario) => Ok(scenario),
        None => bail!(
            "unknown scenario {}; live scenarios: {}",
            opts.scenario,
            crate::scenario::SCENARIOS.join(", ")
        ),
    }
}

fn resolve_sides(opts: &AbOptions) -> Result<(SideBuild, SideBuild)> {
    if opts.no_build {
        return Ok((
            SideBuild {
                rev: opts.a.clone(),
                sha256: "unbuilt".into(),
                path: PathBuf::from(&opts.a),
            },
            SideBuild {
                rev: opts.b.clone(),
                sha256: "unbuilt".into(),
                path: PathBuf::from(format!("{}#side-b", opts.b)),
            },
        ));
    }
    let built_a = build_revision(&build_request(opts, &opts.a))?;
    if opts.control {
        let copies = built_a
            .path
            .parent()
            .context("engine path has no parent")?
            .join("copies");
        let (path_a, path_b, sha256) = distinct_copies(&built_a.path, &copies)?;
        return Ok((
            SideBuild {
                rev: built_a.sha.clone(),
                sha256: sha256.clone(),
                path: path_a,
            },
            SideBuild {
                rev: built_a.sha,
                sha256,
                path: path_b,
            },
        ));
    }
    let built_b = build_revision(&build_request(opts, &opts.b))?;
    Ok((
        SideBuild {
            rev: built_a.sha,
            sha256: built_a.sha256,
            path: built_a.path,
        },
        SideBuild {
            rev: built_b.sha,
            sha256: built_b.sha256,
            path: built_b.path,
        },
    ))
}

fn build_request(opts: &AbOptions, rev: &str) -> BuildRequest {
    BuildRequest {
        rev: rev.to_string(),
        repo: opts.repo.clone(),
        zb_root: opts.zb_root.clone(),
        worktree_parent: default_worktree_parent(),
        target_parent: default_target_parent(),
        jobserver_fifo: default_jobserver_fifo(),
        path_override: None,
    }
}

#[derive(Deserialize)]
struct InjectFile {
    #[serde(default = "default_fixture")]
    fixture_id: String,
    #[serde(default)]
    variant: Value,
    #[serde(default)]
    variant_key: Option<String>,
    #[serde(rename = "A", default)]
    a: Vec<InjectTrial>,
    #[serde(rename = "B", default)]
    b: Vec<InjectTrial>,
}

#[derive(Deserialize)]
struct InjectTrial {
    #[serde(default)]
    metrics: TrialMetrics,
    #[serde(default)]
    counters_delta: BTreeMap<String, f64>,
    #[serde(default = "default_checks")]
    checks: Vec<Check>,
    #[serde(default = "default_true")]
    valid: bool,
    #[serde(default)]
    invalid_reason: Option<String>,
    #[serde(default)]
    noisy: bool,
}

fn default_fixture() -> String {
    "fake".into()
}
fn default_true() -> bool {
    true
}
fn default_checks() -> Vec<Check> {
    vec![Check {
        name: "status_complete".into(),
        ok: true,
        detail: None,
    }]
}

fn load_inject(scenario: &str, path: &Path) -> Result<FakeScenario> {
    let file: InjectFile = read_json(path)?;
    let variant = if file.variant.is_null() {
        json!({"name": "injected"})
    } else {
        file.variant
    };
    let variant_key = file
        .variant_key
        .filter(|key| !key.is_empty())
        .unwrap_or_else(|| "injected".into());
    let mut fake = FakeScenario {
        id: scenario.to_string(),
        variant,
        variant_key,
        fixture_id: file.fixture_id,
        queues: HashMap::new(),
        seen: Vec::new(),
    };
    for (side, trials) in [(Side::A, file.a), (Side::B, file.b)] {
        for trial in trials {
            fake.push(
                side,
                TrialBody {
                    metrics: trial.metrics,
                    counters_delta: trial.counters_delta,
                    checks: trial.checks,
                    valid: trial.valid,
                    invalid_reason: trial.invalid_reason,
                    noisy: trial.noisy,
                    containment: "scope".into(),
                    times_ms: TimesMs::default(),
                    window_s: 1.0,
                },
            );
        }
    }
    Ok(fake)
}

/// Quiet-machine snapshot. Sleeps five seconds. Recorded, not an abort,
/// except a governor change checked by the caller.
pub fn collect_preflight() -> Preflight {
    let clk = clk_tck();
    let first = fs::read_to_string("/proc/stat").unwrap_or_default();
    let engine = crate::detect_real_engine();
    let engine_first = engine
        .as_ref()
        .and_then(|eng| fs::read_to_string(format!("/proc/{}/stat", eng.pid)).ok())
        .as_deref()
        .and_then(stat_ticks);
    std::thread::sleep(Duration::from_secs(5));
    let second = fs::read_to_string("/proc/stat").unwrap_or_default();
    let outside = match (parse_proc_stat_busy(&first), parse_proc_stat_busy(&second)) {
        (Some(a), Some(b)) if b >= a => cpu_pct_of_core(b - a, clk, 5.0),
        _ => 0.0,
    };
    let real_engine_cpu_pct = engine.as_ref().and_then(|eng| {
        let after = fs::read_to_string(format!("/proc/{}/stat", eng.pid)).ok()?;
        let after = stat_ticks(&after)?;
        let before = engine_first?;
        if after >= before {
            Some(cpu_pct_of_core(after - before, clk, 5.0))
        } else {
            None
        }
    });
    let rustc_outside = count_comm("rustc");
    Preflight {
        outside_cpu_pct: outside,
        real_engine_pid: engine.as_ref().map(|eng| eng.pid),
        real_engine_cpu_pct,
        rustc_outside,
        quiet: outside < 50.0 && rustc_outside == 0,
    }
}

pub fn cpu_pct_of_core(ticks_delta: u64, clk_tck: u64, wall_secs: f64) -> f64 {
    if clk_tck == 0 || wall_secs <= 0.0 {
        return 0.0;
    }
    ticks_delta as f64 / clk_tck as f64 / wall_secs * 100.0
}

/// Busy jiffies from a `/proc/stat` snapshot: user + nice + system + irq + softirq + steal.
pub fn parse_proc_stat_busy(text: &str) -> Option<u64> {
    let line = text.lines().find(|line| line.starts_with("cpu "))?;
    let nums: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .map(|part| part.parse().ok())
        .collect::<Option<_>>()?;
    if nums.len() < 4 {
        return None;
    }
    let user = nums[0];
    let nice = nums[1];
    let system = nums[2];
    let irq = nums.get(5).copied().unwrap_or(0);
    let soft = nums.get(6).copied().unwrap_or(0);
    let steal = nums.get(7).copied().unwrap_or(0);
    Some(user + nice + system + irq + soft + steal)
}

/// `utime + stime` from `/proc/<pid>/stat`. The command field may contain spaces.
pub fn stat_ticks(text: &str) -> Option<u64> {
    let rest = text.rsplit_once(')')?.1;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(utime + stime)
}

fn clk_tck() -> u64 {
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz <= 0 { 100 } else { hz as u64 }
}

fn count_comm(name: &str) -> u32 {
    let Ok(dir) = fs::read_dir("/proc") else {
        return 0;
    };
    let mut n = 0u32;
    for ent in dir.flatten() {
        let fname = ent.file_name();
        let fname = fname.to_string_lossy();
        if !fname.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(comm) = fs::read_to_string(ent.path().join("comm")) else {
            continue;
        };
        if comm.trim() == name {
            n += 1;
        }
    }
    n
}

pub fn host_fingerprint() -> Fingerprint {
    Fingerprint {
        machine: machine_id(),
        kernel: read_trim("/proc/sys/kernel/osrelease").unwrap_or_else(|| "unknown".into()),
        governor: read_governors(),
        epp: read_epps(),
        bios: bios_id(),
    }
}

fn governor_signature() -> String {
    format!("governor={};epp={}", read_governors(), read_epps())
}

fn read_governors() -> String {
    read_cpu_files("cpufreq/scaling_governor")
}

fn read_epps() -> String {
    read_cpu_files("cpufreq/energy_performance_preference")
}

fn read_cpu_files(rel: &str) -> String {
    let root = Path::new("/sys/devices/system/cpu");
    let mut values = BTreeMap::<String, ()>::new();
    let Ok(dir) = fs::read_dir(root) else {
        return "unknown".into();
    };
    for ent in dir.flatten() {
        let name = ent.file_name();
        let name = name.to_string_lossy();
        let Some(rest) = name.strip_prefix("cpu") else {
            continue;
        };
        if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Some(text) = read_trim(&ent.path().join(rel).to_string_lossy()) {
            values.insert(text, ());
        }
    }
    if values.is_empty() {
        "unknown".into()
    } else {
        values.into_keys().collect::<Vec<_>>().join(",")
    }
}

fn machine_id() -> String {
    let product = read_trim("/sys/class/dmi/id/product_name").unwrap_or_else(|| "unknown".into());
    let host = read_trim("/proc/sys/kernel/hostname").unwrap_or_else(|| "unknown".into());
    format!("{product} {host}")
}

fn bios_id() -> String {
    let version = read_trim("/sys/class/dmi/id/bios_version").unwrap_or_else(|| "unknown".into());
    let date = read_trim("/sys/class/dmi/id/bios_date").unwrap_or_else(|| "unknown".into());
    format!("{version} {date}")
}

fn read_trim(path: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn read_trim_cmd(cmd: &str) -> String {
    std::process::Command::new(cmd)
        .arg("-vV")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn git_head(repo: &Path) -> Result<String> {
    let output = std::process::Command::new("/usr/bin/git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .context("git rev-parse HEAD")?;
    if !output.status.success() {
        bail!("git rev-parse HEAD failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Print the build result. Used by the `build` subcommand.
pub fn cli_build(rev: &str, zb_root: PathBuf, repo: PathBuf) -> Result<BuiltBinary> {
    let built = build_revision(&BuildRequest {
        rev: rev.to_string(),
        repo,
        zb_root,
        worktree_parent: default_worktree_parent(),
        target_parent: default_target_parent(),
        jobserver_fifo: default_jobserver_fifo(),
        path_override: None,
    })?;
    println!("sha={}", built.sha);
    println!("sha12={}", built.sha12);
    println!("path={}", built.path.display());
    println!("sha256={}", built.sha256);
    println!("reused={}", built.reused);
    println!("worktree={}", built.worktree.display());
    println!("target={}", built.target_dir.display());
    Ok(built)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = PathBuf::from("/home/user")
            .join("Documents/dev/tmp/zeron-perf/scratch/b6-ab-orchestrator/tests")
            .join(format!("{name}-{n}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fp() -> Fingerprint {
        Fingerprint {
            machine: "rig".into(),
            kernel: "6.8.0".into(),
            governor: "powersave".into(),
            epp: "balance_power".into(),
            bios: "1.0".into(),
        }
    }

    fn engine(value: f64) -> TrialBody {
        let mut metrics = TrialMetrics::default();
        metrics.cpu_s.engine = value;
        TrialBody::ok(metrics)
    }

    #[test]
    fn flatten_reads_sampler_metrics() {
        let mut sampled = Metrics::default();
        sampled.cpu_s.engine = 3.0;
        sampled.cpu_s.user = 2.0;
        sampled.pss_mib.by_group_peak.insert("claude".into(), 8.0);
        sampled.cg_mem_peak_mib = Some(10.0);
        sampled.real_engine_cpu_s = Some(0.5);
        sampled.psi_cpu = Some(crate::sample::PsiWindow {
            some_avg10: 1.5,
            some_total_us: 20,
            full_avg10: 0.0,
            full_total_us: 0,
        });
        let mut metrics = TrialMetrics::from_sampled(sampled);
        assert!(metrics.flatten().get("energy_j_net").is_none());
        assert!(metrics.flatten().get("instructions_u").is_none());
        metrics.instructions_u = Some(99.0);
        let flat = metrics.flatten();
        assert_eq!(flat["cpu_s.engine"], 3.0);
        assert_eq!(flat["cpu_s.user"], 2.0);
        assert_eq!(flat["pss_mib.group.claude"], 8.0);
        assert_eq!(flat["cg_mem_peak_mib"], 10.0);
        assert_eq!(flat["real_engine_cpu_s"], 0.5);
        assert_eq!(flat["psi_cpu.some_avg10"], 1.5);
        assert_eq!(flat["psi_cpu.some_total_us"], 20.0);
        assert_eq!(flat["instructions_u"], 99.0);
        assert!(flat.get("energy_j_net").is_none());
        let partial: TrialMetrics = serde_json::from_value(json!({
            "cpu_s": {"engine": 4.5},
            "energy_j_net": 1.25
        }))
        .unwrap();
        assert_eq!(partial.cpu_s.engine, 4.5);
        assert_eq!(partial.cpu_s.tree, 0.0);
        assert_eq!(partial.energy_j_net, Some(1.25));
    }

    fn fill(scenario: &mut FakeScenario, side: Side, values: &[f64]) {
        for value in values {
            scenario.push(side, engine(*value));
        }
    }

    fn request(root: &Path, id: &str, tier: Tier, control: bool) -> ProtocolRequest {
        ProtocolRequest {
            zb_root: root.to_path_buf(),
            run_id: id.into(),
            label: format!("ab {id}"),
            tier,
            seed: 7,
            side_a: SideBuild {
                rev: "aaaaaaaaaaaa".into(),
                sha256: "sha-a".into(),
                path: root.join("a"),
            },
            side_b: SideBuild {
                rev: "bbbbbbbbbbbb".into(),
                sha256: "sha-b".into(),
                path: root.join("b"),
            },
            fingerprint: fp(),
            now_unix: stats::now_unix(),
            bench_git_sha: "benchsha".into(),
            rustc: "rustc test".into(),
            loadavg: "0.10 0.10 0.10".into(),
            amd_pstate: "active".into(),
            control,
            preflight: None,
        }
    }

    fn write_noise(root: &Path, scenario: &str, variant: &str, metric: &str, noise: f64) {
        let mut map = BTreeMap::new();
        map.insert(metric.into(), noise);
        store_noise(
            &root.join("noise"),
            scenario,
            variant,
            &fp(),
            stats::now_unix(),
            &map,
        )
        .unwrap();
    }

    #[test]
    fn energy_is_omitted_unless_present() {
        let mut metrics = TrialMetrics::default();
        let json = serde_json::to_value(&metrics).unwrap();
        assert!(json.get("energy_j_net").is_none());
        assert!(metrics.get("energy_j_net").is_none());
        metrics.energy_j_net = Some(1.5);
        assert_eq!(metrics.get("energy_j_net"), Some(1.5));
        let json = serde_json::to_value(&metrics).unwrap();
        assert_eq!(json["energy_j_net"], 1.5);
    }

    #[test]
    fn fake_scenario_covers_order_spread_verdict_and_noise() {
        // High spread on A forces the quick tier from 2 pairs up to the 2× cap.
        let root = scratch("abba");
        write_noise(&root, "fake", "spread", "cpu_s.engine", 5.0);
        let mut scenario = FakeScenario::new("fake", "spread", "fix-spread");
        fill(&mut scenario, Side::A, &[1.0, 100.0, 120.0, 110.0, 110.0]);
        fill(&mut scenario, Side::B, &[1.0, 50.0, 50.0, 50.0, 50.0]);
        // Counter on every trial, including the warm-up, so the queue stays aligned.
        for body in scenario.queues.get_mut(&Side::A).unwrap() {
            body.counters_delta
                .insert("DocReadEntriesCalls".into(), 100.0);
        }
        for body in scenario.queues.get_mut(&Side::B).unwrap() {
            body.counters_delta
                .insert("DocReadEntriesCalls".into(), 40.0);
        }
        let result = execute(&request(&root, "spread", Tier::Quick, false), &mut scenario).unwrap();
        assert_eq!(
            result.pairs_run, 4,
            "spread should add pairs up to 2× quick"
        );
        assert_eq!(result.extra_pairs, 2);
        let seen: Vec<_> = scenario
            .seen
            .iter()
            .map(|slot| (slot.side, slot.pair_index))
            .collect();
        assert_eq!(
            seen,
            vec![
                (Side::A, None),
                (Side::B, None),
                (Side::A, Some(0)),
                (Side::B, Some(0)),
                (Side::B, Some(1)),
                (Side::A, Some(1)),
                (Side::A, Some(2)),
                (Side::B, Some(2)),
                (Side::B, Some(3)),
                (Side::A, Some(3)),
            ]
        );
        for chunk in scenario
            .seen
            .iter()
            .filter(|s| s.pair_index.is_some())
            .collect::<Vec<_>>()
            .chunks(2)
        {
            assert_eq!(chunk[1].order_index, chunk[0].order_index + 1);
        }
        let summary: Value =
            serde_json::from_str(&fs::read_to_string(result.run_dir.join("summary.json")).unwrap())
                .unwrap();
        let row = &summary["fake"]["spread"]["metrics"]["cpu_s.engine"];
        assert_eq!(row["A"]["n"].as_u64(), Some(4));
        assert_eq!(row["A"]["median"], 110.0);
        assert_eq!(row["verdict"], "better");
        assert_eq!(row["noise_pct"], 5.0);
        assert!(row["paired_delta_pct"]["median"].as_f64().unwrap() < 0.0);
        let report = result.report;
        assert!(report.contains(report::TABLE_HEADER), "{report}");
        assert!(report.contains("sha-a"), "{report}");
        assert!(report.contains("sha-b"));
        assert!(report.contains("rig"));
        assert!(report.contains("powersave"));
        assert!(report.contains("fix-spread"));
        assert!(report.contains("tier: quick"));
        assert!(report.contains("invalid: 0"));
        assert!(report.contains("noisy: 0"));
        let engine_line = report
            .lines()
            .find(|line| line.starts_with("| cpu_s.engine |"))
            .unwrap();
        assert!(engine_line.contains("| better |"), "{engine_line}");
        assert!(engine_line.contains('–'), "{engine_line}");
        assert!(!report.contains("energy_j_net"), "{report}");
        let details = report
            .find("<summary>counters</summary>")
            .expect("counters");
        let end = report.find("</details>").unwrap();
        assert!(report[details..end].contains("DocReadEntriesCalls"));
        assert!(!report.contains("S-build"));
        let again = regenerate_report(&result.run_dir).unwrap();
        assert!(again.contains("| better |"));

        // Exactly 10% does not add a pair. The queue has no third pair.
        let root = scratch("boundary");
        let mut scenario = FakeScenario::new("fake", "boundary", "fix-b");
        fill(&mut scenario, Side::A, &[1.0, 95.0, 105.0]);
        fill(&mut scenario, Side::B, &[1.0, 90.0, 90.0]);
        let result = execute(
            &request(&root, "boundary", Tier::Quick, false),
            &mut scenario,
        )
        .unwrap();
        assert_eq!(result.pairs_run, 2);
        assert_eq!(
            scenario
                .seen
                .iter()
                .map(|s| (s.side, s.pair_index))
                .collect::<Vec<_>>(),
            vec![
                (Side::A, None),
                (Side::B, None),
                (Side::A, Some(0)),
                (Side::B, Some(0)),
                (Side::B, Some(1)),
                (Side::A, Some(1)),
            ]
        );

        // Same sign, no overlap, inside the noise floor → within noise.
        let root = scratch("inside");
        write_noise(&root, "fake", "inside", "cpu_s.engine", 5.0);
        let mut scenario = FakeScenario::new("fake", "inside", "fix-i");
        fill(&mut scenario, Side::A, &[0.0, 100.0, 100.0]);
        fill(&mut scenario, Side::B, &[0.0, 99.0, 99.0]);
        let result = execute(&request(&root, "inside", Tier::Quick, false), &mut scenario).unwrap();
        assert!(result.report.lines().any(|line| {
            line.starts_with("| cpu_s.engine |") && line.contains("| within noise |")
        }));

        // Overlapping ranges, small change.
        let root = scratch("overlap");
        write_noise(&root, "fake", "overlap", "cpu_s.engine", 5.0);
        let mut scenario = FakeScenario::new("fake", "overlap", "fix-o");
        fill(&mut scenario, Side::A, &[0.0, 100.0, 102.0]);
        fill(&mut scenario, Side::B, &[0.0, 101.0, 103.0]);
        let result = execute(
            &request(&root, "overlap", Tier::Quick, false),
            &mut scenario,
        )
        .unwrap();
        assert!(result.report.contains("| within noise |"));

        // B is more expensive.
        let root = scratch("worse");
        write_noise(&root, "fake", "worse", "cpu_s.engine", 5.0);
        let mut scenario = FakeScenario::new("fake", "worse", "fix-w");
        fill(&mut scenario, Side::A, &[0.0, 50.0, 50.0]);
        fill(&mut scenario, Side::B, &[0.0, 80.0, 80.0]);
        let result = execute(&request(&root, "worse", Tier::Quick, false), &mut scenario).unwrap();
        let line = result
            .report
            .lines()
            .find(|line| line.starts_with("| cpu_s.engine |"))
            .unwrap();
        assert!(line.contains("| worse |"), "{line}");

        // A failed check forces invalid, and the detail is verbatim.
        let root = scratch("invalid");
        write_noise(&root, "fake", "invalid", "cpu_s.engine", 1.0);
        let mut scenario = FakeScenario::new("fake", "invalid", "fix-x");
        fill(&mut scenario, Side::A, &[0.0, 100.0, 100.0]);
        fill(&mut scenario, Side::B, &[0.0, 50.0, 50.0]);
        let bad = scenario
            .queues
            .get_mut(&Side::B)
            .unwrap()
            .get_mut(1)
            .unwrap();
        bad.checks = vec![Check {
            name: "transcript_sha".into(),
            ok: false,
            detail: Some("hash mismatch: expected abc got def".into()),
        }];
        let result = execute(
            &request(&root, "invalid", Tier::Quick, false),
            &mut scenario,
        )
        .unwrap();
        assert!(result.report.contains("| invalid |"));
        assert!(
            result
                .report
                .contains("hash mismatch: expected abc got def")
        );
        assert!(result.report.contains("invalid: 1"));

        // No control file: the column says unknown and the verdict stays within noise.
        let root = scratch("unknown");
        let mut scenario = FakeScenario::new("fake", "unknown", "fix-u");
        fill(&mut scenario, Side::A, &[0.0, 100.0, 100.0]);
        fill(&mut scenario, Side::B, &[0.0, 50.0, 50.0]);
        let result = execute(
            &request(&root, "unknown", Tier::Quick, false),
            &mut scenario,
        )
        .unwrap();
        assert!(result.report.contains("- noise: unknown (missing)"));
        let line = result
            .report
            .lines()
            .find(|line| line.starts_with("| cpu_s.engine |"))
            .unwrap();
        assert!(line.contains("| unknown |"), "{line}");
        assert!(line.contains("| within noise |"), "{line}");

        // A fresh control is what the A/B run looks up, not a number it invents.
        let root = scratch("lookup");
        let mut control = FakeScenario::new("fake", "lookup", "fix-c");
        fill(&mut control, Side::A, &[0.0, 100.0, 100.0]);
        fill(&mut control, Side::B, &[0.0, 101.0, 110.0]);
        execute(
            &request(&root, "lookup-control", Tier::Quick, true),
            &mut control,
        )
        .unwrap();
        let mut scenario = FakeScenario::new("fake", "lookup", "fix-c");
        fill(&mut scenario, Side::A, &[0.0, 100.0, 100.0]);
        fill(&mut scenario, Side::B, &[0.0, 50.0, 50.0]);
        let result = execute(
            &request(&root, "lookup-ab", Tier::Quick, false),
            &mut scenario,
        )
        .unwrap();
        let summary: Value =
            serde_json::from_str(&fs::read_to_string(result.run_dir.join("summary.json")).unwrap())
                .unwrap();
        // |Δ| of the self-pair is 1% and 10%. Nearest-rank 90th of two samples is 10.
        assert_eq!(
            summary["fake"]["lookup"]["metrics"]["cpu_s.engine"]["noise_pct"],
            10.0
        );
        assert_eq!(
            summary["fake"]["lookup"]["metrics"]["cpu_s.engine"]["verdict"],
            "better"
        );

        // Stale and a kernel change are both unknown.
        let root = scratch("stale");
        let mut map = BTreeMap::new();
        map.insert("cpu_s.engine".into(), 1.0);
        store_noise(
            &root.join("noise"),
            "fake",
            "stale",
            &fp(),
            1_700_000_000 - stats::NOISE_MAX_AGE_SECS - 5,
            &map,
        )
        .unwrap();
        let mut scenario = FakeScenario::new("fake", "stale", "fix-s");
        fill(&mut scenario, Side::A, &[0.0, 100.0, 100.0]);
        fill(&mut scenario, Side::B, &[0.0, 40.0, 40.0]);
        let result = execute(&request(&root, "stale", Tier::Quick, false), &mut scenario).unwrap();
        assert!(result.report.contains("unknown (stale)"));
        assert!(result.report.contains("| within noise |"));

        // S-idle wakeup table, S-mcp PSS table, and energy only when present.
        let root = scratch("idle");
        write_noise(
            &root,
            "S-idle",
            "ui",
            "wakeups_per_s.comm.tokio-runtime-w",
            1.0,
        );
        let mut scenario = FakeScenario::new("S-idle", "ui", "fix-idle");
        for side in [Side::A, Side::B] {
            for value in [0.0, 10.0, 10.0] {
                let mut body = engine(1.0);
                body.metrics.wakeups_per_s.tree = value;
                body.metrics
                    .wakeups_per_s
                    .by_comm
                    .insert("tokio-runtime-w".into(), value);
                scenario.push(side, body);
            }
        }
        let result = execute(&request(&root, "idle", Tier::Quick, false), &mut scenario).unwrap();
        assert!(result.report.contains("### Wakeups by thread"));
        assert!(result.report.contains("tokio-runtime-w"));

        let root = scratch("mcp");
        write_noise(&root, "S-mcp", "fleet", "pss_mib.group.engine", 1.0);
        let mut scenario = FakeScenario::new("S-mcp", "fleet", "fix-mcp");
        for (side, value) in [(Side::A, 300.0), (Side::B, 200.0)] {
            for _ in 0..3 {
                let mut body = engine(1.0);
                body.metrics.pss_mib.peak = value;
                body.metrics
                    .pss_mib
                    .by_group_peak
                    .insert("claude".into(), value);
                scenario.push(side, body);
            }
        }
        let result = execute(&request(&root, "mcp", Tier::Quick, false), &mut scenario).unwrap();
        assert!(result.report.contains("### PSS by group"));
        assert!(result.report.contains("| claude |"));

        let root = scratch("energy");
        write_noise(&root, "fake", "energy", "energy_j_net", 1.0);
        let mut scenario = FakeScenario::new("fake", "energy", "fix-e");
        for value in [0.0, 2.0, 2.0] {
            let mut body = engine(1.0);
            if value > 0.0 {
                body.metrics.energy_j_net = Some(value);
            }
            scenario.push(Side::A, body);
        }
        for value in [0.0, 1.0, 1.0] {
            let mut body = engine(1.0);
            if value > 0.0 {
                body.metrics.energy_j_net = Some(value);
            }
            scenario.push(Side::B, body);
        }
        let result = execute(&request(&root, "energy", Tier::Quick, false), &mut scenario).unwrap();
        assert!(
            result
                .report
                .lines()
                .any(|line| line.starts_with("| energy_j_net |") && line.contains("| better |")),
            "{}",
            result.report
        );

        let root = scratch("cancelled");
        let mut scenario = FakeScenario::new("S-build", "k4", "fix");
        let err = execute(&request(&root, "build", Tier::Quick, false), &mut scenario).unwrap_err();
        assert!(err.to_string().contains("cancelled"), "{err}");
    }

    #[test]
    fn proc_stat_and_tick_parsers() {
        let text = "cpu  100 10 20 1000 5 1 2 3 0 0\ncpu0 1 0 0 0 0 0 0 0 0 0\n";
        assert_eq!(parse_proc_stat_busy(text), Some(100 + 10 + 20 + 1 + 2 + 3));
        assert!((cpu_pct_of_core(100, 100, 1.0) - 100.0).abs() < 1e-9);
        let stat = "123 (comm name) S 1 1 1 0 -1 0 0 0 0 0 40 10 0 0";
        assert_eq!(stat_ticks(stat), Some(50));
    }
}
