//! The main scenarios: S-stream, S-boot, S-idle, S-switch (bench.md 3.4).
//!
//! Each scenario is a [`Workload`]: it names its fixture and drives one engine
//! through a [`TrialCtx`]. [`Live`] wraps a workload into B6's
//! [`crate::ab::Scenario`], so `zeron-bench ab` and `control` run it with the
//! ABBA protocol. S-build is cancelled and absent.
//!
//! One trial: copy the pristine fixture, start the side's engine in a scope,
//! start the sampler, run the workload between a window start and end, read
//! engine counters (when the revision has them), stop the engine, and reduce
//! everything to a [`TrialBody`]. A trial that fails to launch, aborts
//! abnormally, hits the OOM killer or runs on a saturated machine is invalid
//! and is run once more before it is reported.

pub mod boot;
pub mod content;
mod driver;
pub mod fixture;
pub mod idle;
pub mod stream;
pub mod switch;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::ab::{
    Check, LatencyMs, PlannedTrial, RpcRtt, Scenario, Side, StreamLatency, Tier, TimesMs,
    TrialBody, TrialMetrics,
};
use crate::sample::{
    Cgroup, DataDirSizes, Grouping, Handle, Mark, Metrics, SampleSet, Sampler, Window, metrics,
    snapshot_data_dir,
};
use crate::{Engine, Trial, launch, mono_now};

pub use driver::{Canary, Probe, RpcStats, Ui, Watch, percentile};
pub use fixture::{Fixture, Profile};

/// What every trial of one `ab`/`control` run shares.
#[derive(Clone, Debug)]
pub struct RunEnv {
    pub zb_root: PathBuf,
    pub run_id: String,
    pub tier: Tier,
    pub seed: u64,
    pub bench_bin: PathBuf,
    pub replay_bin: PathBuf,
    pub engine_a: PathBuf,
    pub engine_b: PathBuf,
    /// Builds fixtures that are not cached yet (side A by default).
    pub fixture_engine: PathBuf,
    /// Full sha of the pinned fixture revision (the older side of the pair).
    pub fixture_rev: String,
    /// A trial whose machine-wide outside CPU (percent of one core) exceeds
    /// this is flagged noisy.
    pub noisy_outside_cpu_pct: f64,
    /// ...and invalid (re-run once) above this.
    pub invalid_outside_cpu_pct: f64,
}

impl RunEnv {
    pub fn new(
        zb_root: PathBuf,
        run_id: String,
        tier: Tier,
        seed: u64,
        engine_a: PathBuf,
        engine_b: PathBuf,
        bench_bin: PathBuf,
        fixture_rev: String,
    ) -> Result<Self> {
        let replay_bin = bench_bin
            .parent()
            .map(|dir| dir.join("zb-replay"))
            .context("bench binary has no parent")?;
        if !replay_bin.is_file() {
            bail!(
                "zb-replay not found next to the bench binary ({}); build it with cargo build --release -p zeron-bench",
                replay_bin.display()
            );
        }
        let env_f64 = |key: &str, default: f64| {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };
        Ok(Self {
            zb_root,
            run_id,
            tier,
            seed,
            bench_bin,
            replay_bin,
            fixture_engine: engine_a.clone(),
            fixture_rev,
            engine_a,
            engine_b,
            noisy_outside_cpu_pct: env_f64("ZB_NOISY_OUTSIDE_CPU_PCT", 300.0),
            invalid_outside_cpu_pct: env_f64("ZB_INVALID_OUTSIDE_CPU_PCT", 1200.0),
        })
    }

    pub fn engine(&self, side: Side) -> &Path {
        match side {
            Side::A => &self.engine_a,
            Side::B => &self.engine_b,
        }
    }

    /// A probe-socket path short enough for `sun_path` (108 bytes), still under `ZB_ROOT`.
    pub fn short_sock(&self, label: &str, index: u32) -> Result<PathBuf> {
        let dir = self.zb_root.join("p");
        fs::create_dir_all(&dir)?;
        let stamp = format!("{label}/{index}/{}/{}", std::process::id(), mono_now());
        let hash = format!("{:x}", Sha256::digest(stamp.as_bytes()));
        let path = dir.join(format!("{}.sock", &hash[..12]));
        let _ = fs::remove_file(&path);
        Ok(path)
    }
}

/// Tier-aware parameters: `[defaults]` of the scenario's toml, then
/// `[tier.<name>]`, then `key=value` overrides from the command line.
pub fn load_params<T: DeserializeOwned>(
    source: &str,
    tier: Tier,
    overrides: &[(String, String)],
) -> Result<T> {
    let mut root: toml::Table = toml::from_str(source).context("parse scenario toml")?;
    let mut merged = match root.remove("defaults") {
        Some(toml::Value::Table(t)) => t,
        _ => toml::Table::new(),
    };
    if let Some(toml::Value::Table(mut tiers)) = root.remove("tier")
        && let Some(toml::Value::Table(t)) = tiers.remove(tier.as_str())
    {
        merged.extend(t);
    }
    for (key, value) in overrides {
        let parsed = if let Ok(i) = value.parse::<i64>() {
            toml::Value::Integer(i)
        } else if let Ok(f) = value.parse::<f64>() {
            toml::Value::Float(f)
        } else if let Ok(b) = value.parse::<bool>() {
            toml::Value::Boolean(b)
        } else {
            toml::Value::String(value.clone())
        };
        merged.insert(key.clone(), parsed);
    }
    toml::Value::Table(merged)
        .try_into()
        .context("scenario parameters")
}

/// A scenario named on the command line: `stream`, `S-idle`, `S-stream:K=2,watch=one`.
pub struct ScenarioArg {
    pub name: String,
    pub overrides: Vec<(String, String)>,
}

impl ScenarioArg {
    pub fn parse(text: &str) -> Result<Self> {
        let (name, rest) = text.split_once(':').unwrap_or((text, ""));
        let name = name.trim().to_ascii_lowercase();
        let name = name.strip_prefix("s-").unwrap_or(&name).to_string();
        let mut overrides = Vec::new();
        for pair in rest.split(',').filter(|p| !p.trim().is_empty()) {
            let (k, v) = pair
                .split_once('=')
                .with_context(|| format!("scenario option {pair} is not key=value"))?;
            overrides.push((k.trim().to_string(), v.trim().to_string()));
        }
        Ok(Self { name, overrides })
    }
}

/// Names `ab --scenario` accepts for a live scenario.
pub const SCENARIOS: &[&str] = &["S-stream", "S-boot", "S-idle", "S-switch"];

/// The live scenario `name` (with options), or `None` when it is not one of ours.
pub fn build(name: &str, env: RunEnv) -> Result<Option<Box<dyn Scenario>>> {
    let arg = ScenarioArg::parse(name)?;
    let tier = env.tier;
    let env = Arc::new(env);
    let scenario: Box<dyn Scenario> = match arg.name.as_str() {
        "stream" => Box::new(Live::new(stream::Stream::from_params(tier, &arg.overrides)?, env)?),
        "boot" => Box::new(Live::new(boot::Boot::from_params(tier, &arg.overrides)?, env)?),
        "idle" => Box::new(Live::new(idle::Idle::from_params(tier, &arg.overrides)?, env)?),
        "switch" => Box::new(Live::new(switch::Switch::from_params(tier, &arg.overrides)?, env)?),
        _ => return Ok(None),
    };
    Ok(Some(scenario))
}

/// Engine-side counters (U1) read at window edges. `None` on revisions
/// without the `EngineCounters` RPC.
#[derive(Clone, Debug, Default)]
pub struct Counters {
    pub counters: BTreeMap<String, u64>,
    /// Allocator and runtime gauges (end-of-window values).
    pub gauges: BTreeMap<String, f64>,
}

fn parse_counters(value: &Value) -> Counters {
    let mut out = Counters::default();
    if let Some(map) = value.get("counters").and_then(Value::as_object) {
        for (k, v) in map {
            if let Some(n) = v.as_u64() {
                out.counters.insert(k.clone(), n);
            }
        }
    }
    for group in ["malloc", "tokio"] {
        if let Some(map) = value.get(group).and_then(Value::as_object) {
            for (k, v) in map {
                if let Some(n) = v.as_f64() {
                    out.gauges.insert(format!("{group}_{k}"), n);
                }
            }
        }
    }
    out
}

/// What a scenario implements.
#[allow(async_fn_in_trait)]
pub trait Workload {
    const ID: &'static str;
    fn variant(&self) -> Value;
    fn variant_key(&self) -> String;
    fn profile(&self) -> Profile;
    /// Sampler rate: 4 Hz during activity, 1 Hz for idle windows.
    fn sampler_hz(&self) -> f64 {
        4.0
    }
    /// Extra engine environment (`ZB_REPLAY_*`, `ZERON_*`).
    fn engine_env(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    /// Drive the engine. The workload opens and closes the measured window.
    async fn run(&self, ctx: &mut TrialCtx) -> Result<()>;
}

/// State of one running trial, handed to the workload.
pub struct TrialCtx {
    pub env: Arc<RunEnv>,
    pub planned: PlannedTrial,
    pub trial: Trial,
    pub fixture: Arc<Fixture>,
    pub probe: Probe,
    pub stats: Arc<RpcStats>,
    pub engine_pid: u32,
    pub t_spawn: u64,
    pub t_listen: u64,
    pub t_ready: u64,
    /// Tree CPU (µs, cgroup) and cgroup memory peak (bytes) when the launcher returned.
    pub boot_cpu_us: Option<u64>,
    pub boot_mem_peak: Option<u64>,
    pub checks: Vec<Check>,
    pub scenario: BTreeMap<String, f64>,
    pub stream_lat_ms: Vec<f64>,
    pub rtt_ms: Vec<f64>,
    /// Named monotonic stamps; PSS at each is reported as `pss_at_<name>_mib`.
    pub marks: BTreeMap<String, u64>,
    client: Option<Client>,
    cgroup: Option<Cgroup>,
    sampler: Option<Handle>,
    win: WindowState,
}

use crate::rpc::Client;

#[derive(Default)]
struct WindowState {
    start_ns: u64,
    end_ns: u64,
    counters_start: Option<Counters>,
    counters_end: Option<Counters>,
    counters_tried: bool,
    growth_start: Option<DataDirSizes>,
    growth_end: Option<DataDirSizes>,
    rpc_start: BTreeMap<String, (u64, u64)>,
    rpc_end: BTreeMap<String, (u64, u64)>,
    rapl_start: Option<u64>,
    rapl_end: Option<u64>,
}

impl TrialCtx {
    /// Connect the bench's RPC client (idempotent).
    pub async fn connect(&mut self) -> Result<Client> {
        if let Some(c) = &self.client {
            return Ok(c.clone());
        }
        let c = Client::connect(self.trial.port, &self.trial.data).await?;
        self.client = Some(c.clone());
        Ok(c)
    }

    /// Drop the persistent client (headless variants).
    pub fn disconnect(&mut self) {
        self.client = None;
    }

    pub fn check(&mut self, name: &str, ok: bool, detail: impl Into<String>) {
        let detail = detail.into();
        if !ok {
            eprintln!("zeron-bench: CHECK FAILED {name}: {detail}");
        }
        self.checks.push(Check {
            name: name.to_string(),
            ok,
            detail: (!detail.is_empty()).then_some(detail),
        });
    }

    pub fn metric(&mut self, name: &str, value: f64) {
        if value.is_finite() {
            self.scenario.insert(name.to_string(), value);
        }
    }

    /// Milliseconds since the spawn.
    pub fn ms_since_spawn(&self, t_ns: u64) -> f64 {
        t_ns.saturating_sub(self.t_spawn) as f64 / 1e6
    }

    /// Tree CPU in µs now (cgroup), falling back to the engine's own `/proc` times.
    pub fn tree_cpu_us(&self) -> Option<u64> {
        if let Some(cg) = &self.cgroup {
            return cg.snapshot().cpu.map(|c| c.usage_usec);
        }
        let stat = fs::read_to_string(format!("/proc/{}/stat", self.engine_pid)).ok()?;
        let rest = stat.rsplit_once(')')?.1;
        let f: Vec<&str> = rest.split_whitespace().collect();
        let ticks: u64 = [11, 12, 13, 14]
            .iter()
            .filter_map(|i| f.get(*i)?.parse::<u64>().ok())
            .sum();
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
        Some(ticks * 1_000_000 / hz)
    }

    pub fn set_sampler_hz(&self, hz: f64) -> Result<()> {
        self.sampler.as_ref().context("sampler not running")?.set_hz(hz)
    }

    async fn read_counters(&mut self) -> Option<Counters> {
        let client = match &self.client {
            Some(c) => c.clone(),
            None => Client::connect(self.trial.port, &self.trial.data).await.ok()?,
        };
        let reply = tokio::time::timeout(
            Duration::from_secs(10),
            client.call("EngineCounters", json!({})),
        )
        .await
        .ok()?
        .ok()?;
        Some(parse_counters(&reply.0))
    }

    /// Open the measured window: counters, energy, data dir and RPC baselines,
    /// then the sampler's window start.
    pub async fn open_window(&mut self) -> Result<()> {
        self.win.counters_tried = true;
        self.win.counters_start = self.read_counters().await;
        self.win.rapl_start = crate::sample_package().map(|s| s.energy_uj);
        self.win.growth_start = snapshot_data_dir(&self.trial.data).ok();
        self.win.rpc_start = self.stats.snapshot();
        let sampler = self.sampler.as_ref().context("sampler not running")?;
        sampler.mark(Mark::WindowStart)?;
        self.win.start_ns = mono_now();
        Ok(())
    }

    /// Close the window; counters are read after the edge so the call is not measured.
    pub async fn close_window(&mut self) -> Result<()> {
        let sampler = self.sampler.as_ref().context("sampler not running")?;
        sampler.mark(Mark::WindowEnd)?;
        self.win.end_ns = mono_now();
        self.win.rapl_end = crate::sample_package().map(|s| s.energy_uj);
        self.win.growth_end = snapshot_data_dir(&self.trial.data).ok();
        self.win.rpc_end = self.stats.snapshot();
        if self.win.counters_start.is_some() {
            self.win.counters_end = self.read_counters().await;
        }
        Ok(())
    }

    pub fn window_ns(&self) -> (u64, u64) {
        (self.win.start_ns, self.win.end_ns)
    }

    pub fn window_s(&self) -> f64 {
        self.win.end_ns.saturating_sub(self.win.start_ns) as f64 / 1e9
    }
}

/// Final reduction of a trial.
struct Reduced {
    set: SampleSet,
    exit_note: Option<String>,
    oom: bool,
    perf: (Option<f64>, Option<f64>),
    events: Vec<(u64, crate::probe::ProbeEvent)>,
    stopped_ns: u64,
}

fn read_oom_kills(cgroup: &Path) -> u64 {
    fs::read_to_string(cgroup.join("memory.events"))
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|l| l.strip_prefix("oom_kill ")?.trim().parse().ok())
        })
        .unwrap_or(0)
}

fn read_perf(trial: &Trial) -> (Option<f64>, Option<f64>) {
    let path = trial.root.join("perf.json");
    for _ in 0..40 {
        if let Ok(text) = fs::read_to_string(&path)
            && let Ok(v) = serde_json::from_str::<Value>(&text)
        {
            let get = |k: &str| v.get(k).and_then(Value::as_f64);
            return (get("instructions_u"), get("cycles_u"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    (None, None)
}

/// The generic wrapper from [`Workload`] to B6's scenario trait.
pub struct Live<W: Workload> {
    w: W,
    env: Arc<RunEnv>,
    fx: Arc<Fixture>,
    variant: Value,
    key: String,
    fixture_id: String,
}

impl<W: Workload> Live<W> {
    pub fn new(w: W, env: Arc<RunEnv>) -> Result<Self> {
        let variant = w.variant();
        let key = w.variant_key();
        let fx = fixture::resolve(&env, w.profile())?;
        let fixture_id = fx.id.clone();
        Ok(Self {
            w,
            env,
            fx,
            variant,
            key,
            fixture_id,
        })
    }

    async fn attempt(&self, planned: &PlannedTrial, attempt: u32) -> Result<TrialBody> {
        let env = &self.env;
        let fx = Arc::clone(&self.fx);
        let index = planned.order_index + attempt * 500;
        let mut trial = Trial::at(&env.zb_root, &env.run_id, index, planned.side.as_str())?;
        trial.replay_bin = env.replay_bin.clone();
        trial.bench_bin = env.bench_bin.clone();
        trial.probe_sock = env.short_sock(&env.run_id, index)?;
        trial.extra.push(("ZERON_HARNESS".into(), "claude-code".into()));
        trial.extra.push((
            "ZB_REPLAY_STATE_DIR".into(),
            trial.root.join("replay-state").into_os_string(),
        ));
        for (k, v) in self.w.engine_env() {
            trial.extra.push((k.into(), v.into()));
        }
        trial.allocate_port()?;
        trial.prepare()?;
        fixture::install(&fx, &trial)?;
        let probe = Probe::start(&trial.probe_sock)?;
        let engine = launch(env.engine(planned.side), &trial)
            .await
            .context("launch the engine")?;
        let outcome = self.measure(planned, trial.clone(), fx, probe, engine).await;
        // Heavy per-trial state goes; logs and samples stay.
        archive_and_clean(&trial, outcome.is_ok());
        outcome
    }

    async fn measure(
        &self,
        planned: &PlannedTrial,
        trial: Trial,
        fx: Arc<Fixture>,
        probe: Probe,
        engine: Engine,
    ) -> Result<TrialBody> {
        let env = &self.env;
        let cgroup_dir = engine.cgroup.clone();
        let boot_snap = cgroup_dir
            .as_ref()
            .and_then(|dir| Cgroup::open(dir).ok())
            .map(|cg| cg.snapshot());
        let sampler = Sampler::start(
            engine.pid,
            cgroup_dir.as_ref().and_then(|d| Cgroup::open(d).ok()),
            self.w.sampler_hz(),
            &Grouping::default(),
        )?;
        let mut ctx = TrialCtx {
            env: Arc::clone(env),
            planned: planned.clone(),
            trial,
            fixture: fx,
            probe,
            stats: Arc::new(RpcStats::default()),
            engine_pid: engine.pid,
            t_spawn: engine.t_spawn,
            t_listen: engine.t_listen,
            t_ready: engine.t_ready,
            boot_cpu_us: boot_snap.as_ref().and_then(|s| s.cpu.map(|c| c.usage_usec)),
            boot_mem_peak: boot_snap.as_ref().and_then(|s| s.mem_peak),
            checks: Vec::new(),
            scenario: BTreeMap::new(),
            stream_lat_ms: Vec::new(),
            rtt_ms: Vec::new(),
            marks: BTreeMap::new(),
            client: None,
            cgroup: cgroup_dir.as_ref().and_then(|d| Cgroup::open(d).ok()),
            sampler: Some(sampler),
            win: WindowState::default(),
        };
        let ran = self.w.run(&mut ctx).await;
        if let Err(err) = &ran {
            ctx.check("workload", false, format!("{err:#}"));
        }
        let alive = pid_alive(engine.pid);
        ctx.check("engine_alive", alive, "");
        if ctx.win.end_ns == 0 && ran.is_ok() {
            bail!("workload did not close its window");
        }
        if ran.is_err() && ctx.win.end_ns == 0 {
            // Close what the workload left open so the failure still has numbers.
            if ctx.win.start_ns == 0 {
                let _ = ctx.open_window().await;
            }
            let _ = ctx.close_window().await;
        }
        ctx.client = None;
        let oom = cgroup_dir.as_deref().map(read_oom_kills).unwrap_or(0) > 0;
        let sampler = ctx.sampler.take().context("sampler vanished")?;
        let set = tokio::task::spawn_blocking(move || sampler.finish())
            .await
            .context("join sampler")??;
        let stop = engine.stop().await;
        let stopped_ns = mono_now();
        let exit_note = match &stop {
            Ok(report) if matches!(report.how, crate::StopHow::Rpc) => None,
            Ok(report) => Some(format!("engine needed {:?} to stop", report.how)),
            Err(err) => Some(format!("stop failed: {err:#}")),
        };
        let containment = stop
            .as_ref()
            .map(|r| r.containment)
            .unwrap_or("scope")
            .to_string();
        let perf = read_perf(&ctx.trial);
        let events = ctx.probe.finish();
        let reduced = Reduced {
            set,
            exit_note,
            oom,
            perf,
            events,
            stopped_ns,
        };
        self.reduce(ctx, reduced, containment)
    }

    fn reduce(&self, mut ctx: TrialCtx, red: Reduced, containment: String) -> Result<TrialBody> {
        let Reduced {
            mut set,
            exit_note,
            oom,
            perf,
            events,
            stopped_ns,
        } = red;
        let window: Window = set.marked_window().unwrap_or(Window {
            start_ns: ctx.win.start_ns,
            end_ns: ctx.win.end_ns,
        });
        if let (Some(a), Some(b)) = (&ctx.win.growth_start, &ctx.win.growth_end) {
            set.data_dir_growth = b.growth_since(a);
        }
        let (rpc_bytes, rpc_items) = driver::rpc_delta(&ctx.win.rpc_start, &ctx.win.rpc_end);
        set.rpc_bytes_in = rpc_bytes;
        set.rpc_items = rpc_items;
        let mut sampled: Metrics = metrics(&set, window);
        // zb-shim counts every spawn, including ones shorter than a tick.
        if ctx.trial.shims.join("zb-shim").exists() {
            let mut shim = BTreeMap::new();
            for (_, event) in &events {
                if let crate::probe::ProbeEvent::Spawn { prog, t, .. } = event
                    && *t >= window.start_ns
                    && *t <= window.end_ns
                {
                    // zb-replay stands in for the `claude` process.
                    let prog = if prog == "zb-replay" { "claude" } else { prog.as_str() };
                    *shim.entry(prog.to_string()).or_insert(0u64) += 1;
                }
            }
            sampled.spawns = shim;
        }
        let wall = window.end_ns.saturating_sub(window.start_ns) as f64 / 1e9;
        let outside = sampled.outside_cpu_pct;
        let mut m = TrialMetrics::from_sampled(sampled);
        m.instructions_u = perf.0;
        m.cycles_u = perf.1;
        // Package energy is machine-wide and only reported when RAPL is readable.
        if let (Some(a), Some(b)) = (ctx.win.rapl_start, ctx.win.rapl_end)
            && let Some(max) = max_energy_range_uj()
            && let Some(delta) = crate::energy_delta_uj(a, b, max)
        {
            ctx.metric("energy_j", delta as f64 / 1e6);
        }
        // PSS at the window edges, for drift.
        let in_window: Vec<_> = set
            .ticks
            .iter()
            .filter(|t| t.t_ns >= window.start_ns && t.t_ns <= window.end_ns)
            .collect();
        if let (Some(first), Some(last)) = (in_window.first(), in_window.last()) {
            let start = first.rollup.pss_kib as f64 / 1024.0;
            let end = last.rollup.pss_kib as f64 / 1024.0;
            ctx.metric("pss_start_mib", start);
            ctx.metric("pss_drift_mib", end - start);
        }
        for (name, t) in ctx.marks.clone() {
            if let Some(tick) = set.ticks.iter().rev().find(|tick| tick.t_ns <= t) {
                ctx.metric(&format!("pss_at_{name}_mib"), tick.rollup.pss_kib as f64 / 1024.0);
            }
        }
        if let (Some(a), Some(b)) = (
            ctx.scenario.get("pss_at_after12_mib").copied(),
            ctx.scenario.get("pss_at_last_mib").copied(),
        ) {
            ctx.metric("pss_growth_12_to_last_mib", b - a);
        }
        // Per-unit costs: per MB streamed, per minute of idle, per switch.
        if let Some(bytes) = ctx.scenario.get("streamed_text_bytes").copied()
            && bytes > 0.0
        {
            let mb = bytes / 1e6;
            ctx.metric("cpu_s_per_mb.tree", m.sampled.cpu_s.tree / mb);
            ctx.metric("cpu_s_per_mb.engine", m.sampled.cpu_s.engine / mb);
            ctx.metric("write_bytes_per_mb", m.sampled.write_bytes as f64 / mb);
        }
        if let Some(min) = ctx.scenario.get("window_min").copied()
            && min > 0.0
        {
            ctx.metric("cpu_ms_per_min.tree", m.sampled.cpu_s.tree * 1000.0 / min);
            ctx.metric("cpu_ms_per_min.engine", m.sampled.cpu_s.engine * 1000.0 / min);
            ctx.metric("write_bytes_per_min", m.sampled.write_bytes as f64 / min);
            let git = m.sampled.spawns.get("git").copied().unwrap_or(0);
            ctx.metric("spawns_git_per_min", git as f64 / min);
        }
        if let Some(n) = ctx.scenario.get("switches").copied()
            && n > 0.0
        {
            ctx.metric("cpu_s_per_switch.tree", m.sampled.cpu_s.tree / n);
            let git = m.sampled.spawns.get("git").copied().unwrap_or(0);
            ctx.metric("spawns_git_per_switch", git as f64 / n);
        }
        if let Some(peak) = ctx.boot_mem_peak {
            ctx.metric("boot_cg_peak_mib", peak as f64 / 1048576.0);
        }
        // Stream latency from replay write to client receipt.
        if !ctx.stream_lat_ms.is_empty() {
            let v = &ctx.stream_lat_ms;
            m.latency_ms = LatencyMs {
                stream: Some(StreamLatency {
                    n: v.len() as u64,
                    p50: percentile(v, 50.0),
                    p90: percentile(v, 90.0),
                    p99: percentile(v, 99.0),
                    max: v.iter().copied().fold(0.0, f64::max),
                }),
                rpc_rtt: None,
            };
        }
        if !ctx.rtt_ms.is_empty() {
            let v = &ctx.rtt_ms;
            m.latency_ms.rpc_rtt = Some(RpcRtt {
                p50: percentile(v, 50.0),
                p99: percentile(v, 99.0),
            });
        }
        let mut counters_delta = BTreeMap::new();
        if let (Some(a), Some(b)) = (&ctx.win.counters_start, &ctx.win.counters_end) {
            for (name, end) in &b.counters {
                let start = a.counters.get(name).copied().unwrap_or(0);
                counters_delta.insert(name.clone(), end.saturating_sub(start) as f64);
            }
            for (name, value) in &b.gauges {
                ctx.scenario.insert(format!("engine_{name}"), *value);
            }
        }
        m.scenario = ctx.scenario.clone();
        let mut valid = true;
        let mut reason: Option<String> = None;
        if let Some(note) = exit_note {
            valid = false;
            reason = Some(note);
        }
        if oom {
            valid = false;
            reason = Some("OOM kill in the trial scope".into());
        }
        if outside > ctx.env.invalid_outside_cpu_pct {
            valid = false;
            reason = Some(format!(
                "machine saturated: outside CPU {outside:.0}% of a core"
            ));
        }
        if wall <= 0.0 {
            valid = false;
            reason = Some("empty measurement window".into());
        }
        let noisy = outside > ctx.env.noisy_outside_cpu_pct;
        let rel = |t: u64| t.saturating_sub(ctx.t_spawn) / 1_000_000;
        let times = TimesMs {
            spawn: 0,
            listen: rel(ctx.t_listen),
            ready: rel(ctx.t_ready),
            window_start: rel(window.start_ns),
            window_end: rel(window.end_ns),
            stopped: rel(stopped_ns),
        };
        self.write_artifacts(&ctx, &set, &events, &m);
        eprintln!(
            "zeron-bench: {} {} trial {} done: window {:.1}s, tree cpu {:.2}s, pss peak {:.0} MiB, outside {:.0}%{}{}",
            W::ID,
            ctx.planned.side.as_str(),
            ctx.planned.order_index,
            wall,
            m.sampled.cpu_s.tree,
            m.sampled.pss_mib.peak,
            outside,
            if noisy { " NOISY" } else { "" },
            reason
                .as_deref()
                .map(|r| format!(" INVALID({r})"))
                .unwrap_or_default()
        );
        Ok(TrialBody {
            metrics: m,
            counters_delta,
            checks: ctx.checks,
            valid,
            invalid_reason: reason,
            noisy,
            containment,
            times_ms: times,
            window_s: wall,
        })
    }

    fn write_artifacts(
        &self,
        ctx: &TrialCtx,
        set: &SampleSet,
        events: &[(u64, crate::probe::ProbeEvent)],
        m: &TrialMetrics,
    ) {
        let root = &ctx.trial.root;
        let samples: String = set.ticks.iter().map(|t| t.ndjson_line() + "\n").collect();
        let _ = fs::write(root.join("samples.ndjson"), samples);
        let probe: String = events
            .iter()
            .filter_map(|(t, e)| serde_json::to_string(&json!({"recv": t, "event": e})).ok())
            .map(|l| l + "\n")
            .collect();
        let _ = fs::write(root.join("probe.ndjson"), probe);
        let counters = |c: &Option<Counters>| match c {
            Some(c) => json!({"counters": c.counters, "gauges": c.gauges}),
            None => Value::Null,
        };
        let _ = fs::write(
            root.join("counters.start.json"),
            counters(&ctx.win.counters_start).to_string(),
        );
        let _ = fs::write(
            root.join("counters.end.json"),
            counters(&ctx.win.counters_end).to_string(),
        );
        let _ = fs::write(
            root.join("scenario.json"),
            serde_json::to_vec_pretty(&json!({
                "scenario": W::ID, "variant": self.variant, "fixture": self.fixture_id,
                "metrics": m.scenario, "checks": ctx.checks.iter().map(|c| json!({"name": c.name, "ok": c.ok, "detail": c.detail})).collect::<Vec<_>>(),
            }))
            .unwrap_or_default(),
        );
    }
}

fn max_energy_range_uj() -> Option<u64> {
    fs::read_to_string("/sys/class/powercap/intel-rapl:0/max_energy_range_uj")
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn pid_alive(pid: u32) -> bool {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next().map(|s| s != "Z"))
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// Keep logs and artifacts, drop the (large) data dir. A failed trial keeps its data.
fn archive_and_clean(trial: &Trial, ok: bool) {
    let logs = trial.data.join("logs");
    if logs.is_dir() {
        let _ = fs::rename(&logs, trial.root.join("engine-logs"));
    }
    if ok {
        let _ = fs::remove_dir_all(&trial.data);
        let _ = fs::remove_dir_all(&trial.worktrees);
        let _ = fs::remove_dir_all(&trial.home);
        let _ = fs::remove_dir_all(trial.root.join("replay-state"));
        let _ = fs::remove_dir_all(trial.root.join("scripts"));
    }
}

fn invalid_body(reason: String) -> TrialBody {
    let mut body = TrialBody::ok(TrialMetrics::default());
    body.valid = false;
    body.invalid_reason = Some(reason.clone());
    body.checks = vec![Check {
        name: "trial_completed".into(),
        ok: false,
        detail: Some(reason),
    }];
    body
}

impl<W: Workload> Scenario for Live<W> {
    fn id(&self) -> &str {
        W::ID
    }

    fn variant(&self) -> &Value {
        &self.variant
    }

    fn variant_key(&self) -> &str {
        &self.key
    }

    fn fixture_id(&self) -> &str {
        &self.fixture_id
    }

    fn run_trial(&mut self, planned: &PlannedTrial) -> Result<TrialBody> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .context("start the trial runtime")?;
        let mut last = None;
        for attempt in 0..2u32 {
            eprintln!(
                "zeron-bench: {} {} trial {} (pair {:?}) attempt {}",
                W::ID,
                planned.side.as_str(),
                planned.order_index,
                planned.pair_index,
                attempt + 1
            );
            let body = match rt.block_on(self.attempt(planned, attempt)) {
                Ok(body) => body,
                Err(err) => {
                    eprintln!("zeron-bench: trial failed: {err:#}");
                    invalid_body(format!("{err:#}"))
                }
            };
            let good = body.valid && body.checks.iter().all(|c| c.ok);
            if good {
                return Ok(body);
            }
            eprintln!(
                "zeron-bench: trial not clean ({}); {}",
                body.invalid_reason.as_deref().unwrap_or("check failed"),
                if attempt == 0 { "re-running once" } else { "keeping it" }
            );
            last = Some(body);
        }
        rt.shutdown_background();
        last.context("no attempt ran")
    }
}
