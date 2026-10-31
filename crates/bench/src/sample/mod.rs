//! `/proc` and cgroup sampler for one process tree.
//!
//! The sampler thread stays outside the measured cgroup. It keeps each pid's
//! proc files open and re-reads them, which is what keeps a 4 Hz pass over a
//! 50-process tree under 1% of a core. Counters are cumulative. [`metrics`]
//! subtracts the sample at the start of the window from the sample at the end,
//! so a process that exits mid-window still contributes the last value read
//! for I/O, wakeups and per-group CPU.
//!
//! Tree CPU prefers the cgroup's `cpu.stat`, which includes tasks that exited
//! without being sampled. Without a cgroup it is the live pids' `utime+stime`
//! plus the root pid's `cutime+cstime` (reaped children). Per-group CPU is
//! always the sum of each pid's own `utime+stime`, with the last sample kept
//! after the pid disappears.
//!
//! Linux only. RAPL is not read.

#![cfg(unix)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub mod cgroup;
pub mod proc;

pub use cgroup::Cgroup;
pub use proc::{
    InotifyCount, InotifyWatch, count_arena_heaps, inotify_summary, parse_inotify_fdinfo,
};

use cgroup::{CgroupSnap, Pressure};
use proc::{
    FdScan, Schedstat, StatusSwitches, inotify_summary as summarize_inotify, open_at, open_read,
    parse_children, parse_io_bytes, parse_machine_cpu, parse_schedstat_bytes,
    parse_smaps_rollup_bytes, parse_stat, parse_stat_view, parse_status_switches_bytes, reread,
    scan_fds, text_of,
};

const GROUP_ENGINE: &str = "engine";
const GROUP_REPLAY: &str = "replay";
const GROUP_CLAUDE: &str = "claude";
const GROUP_NPM: &str = "npm-wrapper";
const GROUP_BROWSER: &str = "browser";
const GROUP_GIT: &str = "git";
const GROUP_BUILD: &str = "build";
const GROUP_OTHER: &str = "other";

/// Which edge of the measured window [`Handle::mark`] should take.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mark {
    WindowStart,
    WindowEnd,
}

/// A mark the sampler took, timestamped with the sample it just read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marked {
    pub kind: Mark,
    pub t_ns: u64,
}

/// Inclusive window passed to [`metrics`]. Nanoseconds are `CLOCK_MONOTONIC`,
/// the same clock as [`crate::mono_now`] and [`Marked::t_ns`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub start_ns: u64,
    pub end_ns: u64,
}

/// One configured MCP command. The first rule whose needle is contained in
/// the cmdline (or the exe path) wins, and the group name is `mcp:<name>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpRule {
    pub name: String,
    pub needles: Vec<String>,
}

/// How a pid is labelled. The root pid is always `engine`. After that the
/// first match wins: `replay`, `claude`, `mcp:<name>`, `npm-wrapper`,
/// `browser`, `git`, `build`, then `other`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grouping {
    /// Basenames, or absolute paths, of the real `claude` binary.
    /// Empty matches the basename `claude`.
    pub claude: Vec<String>,
    pub mcp: Vec<McpRule>,
}

impl Default for Grouping {
    fn default() -> Self {
        Self {
            claude: Vec::new(),
            mcp: vec![
                McpRule {
                    name: "chrome-devtools".into(),
                    needles: vec!["chrome-devtools-mcp".into()],
                },
                McpRule {
                    name: "playwright".into(),
                    needles: vec!["@playwright/mcp".into()],
                },
                McpRule {
                    name: "zeron".into(),
                    needles: vec!["zeron mcp".into(), "zeron-engine mcp".into()],
                },
            ],
        }
    }
}

impl Grouping {
    pub fn classify(&self, root_pid: u32, pid: u32, exe: &str, cmdline: &str) -> String {
        if pid == root_pid {
            return GROUP_ENGINE.into();
        }
        let exe = exe.strip_suffix(" (deleted)").unwrap_or(exe);
        let mut base = basename(exe);
        if base.is_empty() {
            base = basename(first_token(cmdline));
        }
        if base == "zb-replay" || cmdline.contains("zb-replay") {
            return GROUP_REPLAY.into();
        }
        if self.is_claude(exe, cmdline, base) {
            return GROUP_CLAUDE.into();
        }
        for rule in &self.mcp {
            if rule.needles.iter().any(|needle| {
                !needle.is_empty() && (cmdline.contains(needle) || exe.contains(needle))
            }) {
                return format!("mcp:{}", rule.name);
            }
        }
        if is_npm_wrapper(base, cmdline) {
            return GROUP_NPM.into();
        }
        if is_browser(base) {
            return GROUP_BROWSER.into();
        }
        if base == "git" {
            return GROUP_GIT.into();
        }
        if matches!(base, "cargo" | "rustc" | "cc" | "ld") {
            return GROUP_BUILD.into();
        }
        GROUP_OTHER.into()
    }

    fn is_claude(&self, exe: &str, cmdline: &str, base: &str) -> bool {
        if self.claude.is_empty() {
            return base == "claude";
        }
        self.claude.iter().any(|needle| {
            if needle.contains('/') {
                exe == needle || cmdline.contains(needle)
            } else {
                base == needle
                    || cmdline
                        .split_whitespace()
                        .any(|token| basename(token) == needle)
            }
        })
    }
}

/// Byte sizes of a trial data directory, classified the way `trial.json` reports growth.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataDirSizes {
    pub sqlite: u64,
    pub wal: u64,
    pub journals: u64,
    pub logs: u64,
    pub previews_json: u64,
    pub other: u64,
}

/// `after - before` per class, saturating at zero when a class shrinks.
pub type DataDirGrowth = DataDirSizes;

impl DataDirSizes {
    pub fn growth_since(&self, before: &Self) -> Self {
        Self {
            sqlite: self.sqlite.saturating_sub(before.sqlite),
            wal: self.wal.saturating_sub(before.wal),
            journals: self.journals.saturating_sub(before.journals),
            logs: self.logs.saturating_sub(before.logs),
            previews_json: self.previews_json.saturating_sub(before.previews_json),
            other: self.other.saturating_sub(before.other),
        }
    }
}

/// Classify regular files under `root`. Symlinks are not followed and not counted.
pub fn snapshot_data_dir(root: &Path) -> io::Result<DataDirSizes> {
    let mut sizes = DataDirSizes::default();
    walk_data_dir(root, root, &mut sizes)?;
    Ok(sizes)
}

fn walk_data_dir(root: &Path, dir: &Path, sizes: &mut DataDirSizes) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let meta = entry.metadata()?;
        let file_type = meta.file_type();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            walk_data_dir(root, &path, sizes)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        add_file(sizes, root, &path, meta.len());
    }
    Ok(())
}

fn add_file(sizes: &mut DataDirSizes, root: &Path, path: &Path, len: u64) {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or("");
    let in_dir = |component: &str| rel.components().any(|part| part.as_os_str() == component);
    if in_dir("journals") {
        sizes.journals = sizes.journals.saturating_add(len);
    } else if in_dir("logs") {
        sizes.logs = sizes.logs.saturating_add(len);
    } else if name == "previews.json" {
        sizes.previews_json = sizes.previews_json.saturating_add(len);
    } else if name.ends_with("-wal") {
        sizes.wal = sizes.wal.saturating_add(len);
    } else if name == "docs.sqlite3" || name.ends_with(".sqlite3") {
        sizes.sqlite = sizes.sqlite.saturating_add(len);
    } else {
        sizes.other = sizes.other.saturating_add(len);
    }
}

/// One live process at one tick. Cumulative counters are since process start.
///
/// Memory fields are bytes. `utime` and `stime` are microseconds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcSnap {
    pub pid: u32,
    pub ppid: u32,
    pub group: String,
    pub comm: String,
    pub pss: u64,
    pub pss_anon: u64,
    pub pss_file: u64,
    pub swap_pss: u64,
    pub rss: u64,
    pub utime: u64,
    pub stime: u64,
    pub threads: u32,
    pub wb: u64,
    pub syscw: u64,
    pub sched_pcount: u64,
    pub vcsw: u64,
    pub nvcsw: u64,
}

/// Cumulative tree counters at one tick. [`metrics`] subtracts two of these.
#[derive(Clone, Debug, Default)]
pub struct Rollup {
    pub cpu_cgroup_us: Option<u64>,
    pub cpu_cgroup_user_us: Option<u64>,
    pub cpu_cgroup_system_us: Option<u64>,
    /// Live `utime+stime` plus the root's `cutime+cstime`, in microseconds.
    pub cpu_proc_us: u64,
    pub cpu_proc_user_us: u64,
    pub cpu_proc_system_us: u64,
    /// Root pid's own `utime+stime`, excluding children.
    pub cpu_engine_us: u64,
    pub cpu_by_group_us: BTreeMap<String, u64>,
    pub pss_kib: u64,
    pub pss_anon_kib: u64,
    pub pss_file_kib: u64,
    pub swap_pss_kib: u64,
    pub pss_by_group_kib: BTreeMap<String, u64>,
    pub pss_anon_by_group_kib: BTreeMap<String, u64>,
    pub pss_file_by_group_kib: BTreeMap<String, u64>,
    pub swap_pss_by_group_kib: BTreeMap<String, u64>,
    pub sched_pcount: u64,
    pub wake_by_comm: BTreeMap<String, u64>,
    pub vcsw: u64,
    pub nvcsw: u64,
    pub write_bytes: u64,
    pub wchar: u64,
    pub syscw: u64,
    pub threads: u32,
    pub thread_names: BTreeMap<String, u32>,
    pub arena_heaps: u32,
    pub fds: u32,
    pub inotify_watches: u32,
    pub inotify_dup: u32,
    pub spawns: BTreeMap<String, u64>,
    pub cg_mem_current: Option<u64>,
    pub cg_mem_peak: Option<u64>,
    pub cg_anon: Option<u64>,
    pub psi_cpu: Option<Pressure>,
    pub psi_mem: Option<Pressure>,
    pub proc_busy_us: u64,
    pub real_engine_cpu_us: Option<u64>,
}

/// One sampler tick. `tree_*` is the authoritative cumulative shown in `samples.ndjson`.
#[derive(Clone, Debug)]
pub struct Tick {
    pub t_ns: u64,
    pub tree_cpu_us: u64,
    pub tree_mem_cur: u64,
    pub tree_anon: u64,
    pub tree_pids: u32,
    pub procs: Vec<ProcSnap>,
    pub rollup: Rollup,
}

impl Tick {
    /// One `samples.ndjson` line: `{t, tree:{cpu_us, mem_cur, anon, pids}, procs:[…]}`.
    pub fn ndjson_line(&self) -> String {
        let procs = self
            .procs
            .iter()
            .map(|proc| {
                serde_json::json!({
                    "pid": proc.pid,
                    "ppid": proc.ppid,
                    "group": proc.group,
                    "comm": proc.comm,
                    "pss": proc.pss,
                    "pss_anon": proc.pss_anon,
                    "swap_pss": proc.swap_pss,
                    "rss": proc.rss,
                    "utime": proc.utime,
                    "stime": proc.stime,
                    "threads": proc.threads,
                    "wb": proc.wb,
                    "syscw": proc.syscw,
                    "sched_pcount": proc.sched_pcount,
                    "vcsw": proc.vcsw,
                    "nvcsw": proc.nvcsw,
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "t": self.t_ns,
            "tree": {
                "cpu_us": self.tree_cpu_us,
                "mem_cur": self.tree_mem_cur,
                "anon": self.tree_anon,
                "pids": self.tree_pids,
            },
            "procs": procs,
        })
        .to_string()
    }
}

/// Everything the sampler observed. Caller-filled fields are copied into [`Metrics`].
#[derive(Clone, Debug)]
pub struct SampleSet {
    pub root_pid: u32,
    pub hz: f64,
    /// `"scope"` when a cgroup was supplied, otherwise `"pgroup"`.
    pub containment: &'static str,
    pub clk_tck: u64,
    pub ticks: Vec<Tick>,
    pub marks: Vec<Marked>,
    /// Delta the scenario computed. [`metrics`] copies it; the sampler does not
    /// watch the data directory itself.
    pub data_dir_growth: DataDirGrowth,
    /// Spawn events from `zb-shim`, already limited to the caller's window.
    pub extra_spawns: BTreeMap<String, u64>,
    pub rpc_bytes_in: BTreeMap<String, u64>,
    pub rpc_items: BTreeMap<String, u64>,
}

impl SampleSet {
    /// The latest `WindowStart` and the `WindowEnd` after it.
    pub fn marked_window(&self) -> Option<Window> {
        let start = self
            .marks
            .iter()
            .rev()
            .find(|mark| mark.kind == Mark::WindowStart)?;
        let end = self
            .marks
            .iter()
            .rev()
            .find(|mark| mark.kind == Mark::WindowEnd && mark.t_ns >= start.t_ns)?;
        Some(Window {
            start_ns: start.t_ns,
            end_ns: end.t_ns,
        })
    }
}

/// Tree and per-group CPU time over the window, in seconds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CpuSeconds {
    pub tree: f64,
    pub engine: f64,
    pub user: f64,
    pub system: f64,
    pub by_group: BTreeMap<String, f64>,
}

/// `cpu_s / wall * 100`. 100 is one fully busy core.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CpuPct {
    pub tree: f64,
    pub engine: f64,
}

/// Proportional set over the live tree, mebibytes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PssMib {
    pub peak: f64,
    pub mean: f64,
    pub end: f64,
    pub by_group_peak: BTreeMap<String, f64>,
    pub by_group_end: BTreeMap<String, f64>,
}

/// Peak, mean and end for one memory class, in mebibytes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MemLevel {
    pub peak: f64,
    pub mean: f64,
    pub end: f64,
    pub by_group_peak: BTreeMap<String, f64>,
}

/// Anonymous cgroup memory, mebibytes. Absent when no cgroup was sampled.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CgAnon {
    pub peak: f64,
    pub mean: f64,
    pub end: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Wakeups {
    pub tree: f64,
    pub by_comm: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CtxRate {
    pub vol: f64,
    pub invol: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadCount {
    pub max: u32,
    pub end: u32,
    pub by_comm: BTreeMap<String, u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InotifyMetrics {
    pub watches: u32,
    pub dup: u32,
}

/// PSI over the window. `avg10` is the gauge at the end. `total_us` is the delta.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PsiWindow {
    pub some_avg10: f64,
    pub some_total_us: u64,
    pub full_avg10: f64,
    pub full_total_us: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcMetrics {
    pub bytes_in: BTreeMap<String, u64>,
    pub items: BTreeMap<String, u64>,
}

/// §2.1 metrics for one window.
///
/// `instructions_u`, `cycles_u` and `energy_j_net` are not filled here.
/// Instruction counts belong to the perf wrapper. Package energy is not read
/// (RAPL stays unreadable, and nothing in this sampler asks for root).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    pub cpu_s: CpuSeconds,
    pub cpu_pct_core: CpuPct,
    pub pss_mib: PssMib,
    pub pss_anon_mib: MemLevel,
    pub pss_file_mib: MemLevel,
    pub swap_pss_mib: MemLevel,
    pub cg_mem_peak_mib: Option<f64>,
    pub cg_anon_mib: Option<CgAnon>,
    pub wakeups_per_s: Wakeups,
    pub ctx_per_s: CtxRate,
    pub write_bytes: u64,
    pub wchar: u64,
    pub syscw: u64,
    pub data_dir_growth: DataDirGrowth,
    pub threads: ThreadCount,
    pub arena_heaps: u32,
    pub fds: u32,
    pub inotify: InotifyMetrics,
    pub spawns: BTreeMap<String, u64>,
    pub rpc: RpcMetrics,
    pub outside_cpu_pct: f64,
    /// Own CPU of the user's live engine, when `~/.zeron/engine.lock` names one.
    /// Read-only. `None` when no live engine was detected.
    pub real_engine_cpu_s: Option<f64>,
    pub real_engine_cpu_pct: Option<f64>,
    pub psi_cpu: Option<PsiWindow>,
    pub psi_mem: Option<PsiWindow>,
}

/// Reduce `set` to the §2.1 metrics for `window`.
///
/// Ticks with `start_ns <= t <= end_ns` form the window. Gauges (PSS, fds,
/// inotify, arenas, thread names) use those ticks. Rates and byte deltas are
/// `last - first` over the same ticks, divided by the window length.
pub fn metrics(set: &SampleSet, window: Window) -> Metrics {
    let mut out = Metrics {
        data_dir_growth: set.data_dir_growth.clone(),
        rpc: RpcMetrics {
            bytes_in: set.rpc_bytes_in.clone(),
            items: set.rpc_items.clone(),
        },
        ..Metrics::default()
    };
    let Some((first_i, last_i)) = window_bounds(&set.ticks, window) else {
        return out;
    };
    let first = &set.ticks[first_i];
    let last = &set.ticks[last_i];
    let slice = &set.ticks[first_i..=last_i];
    let wall_s = window_seconds(window);
    let (tree_us, user_us, system_us) = cpu_delta(&first.rollup, &last.rollup);
    out.cpu_s.tree = us_to_s(tree_us);
    out.cpu_s.engine = us_to_s(
        last.rollup
            .cpu_engine_us
            .saturating_sub(first.rollup.cpu_engine_us),
    );
    out.cpu_s.user = us_to_s(user_us);
    out.cpu_s.system = us_to_s(system_us);
    out.cpu_s.by_group = map_us_delta(&first.rollup.cpu_by_group_us, &last.rollup.cpu_by_group_us);
    if wall_s > 0.0 {
        out.cpu_pct_core.tree = out.cpu_s.tree / wall_s * 100.0;
        out.cpu_pct_core.engine = out.cpu_s.engine / wall_s * 100.0;
    }
    out.pss_mib = level_total(
        slice,
        |tick| tick.rollup.pss_kib,
        |tick| &tick.rollup.pss_by_group_kib,
    );
    out.pss_anon_mib = level_parts(
        slice,
        |tick| tick.rollup.pss_anon_kib,
        |tick| &tick.rollup.pss_anon_by_group_kib,
    );
    out.pss_file_mib = level_parts(
        slice,
        |tick| tick.rollup.pss_file_kib,
        |tick| &tick.rollup.pss_file_by_group_kib,
    );
    out.swap_pss_mib = level_parts(
        slice,
        |tick| tick.rollup.swap_pss_kib,
        |tick| &tick.rollup.swap_pss_by_group_kib,
    );
    out.cg_mem_peak_mib = slice
        .iter()
        .filter_map(|tick| tick.rollup.cg_mem_peak)
        .max()
        .map(bytes_to_mib);
    out.cg_anon_mib = cg_anon(slice);
    let sched = last
        .rollup
        .sched_pcount
        .saturating_sub(first.rollup.sched_pcount);
    out.wakeups_per_s.tree = per_second(sched, wall_s);
    out.wakeups_per_s.by_comm = map_rate(
        &first.rollup.wake_by_comm,
        &last.rollup.wake_by_comm,
        wall_s,
    );
    out.ctx_per_s.vol = per_second(last.rollup.vcsw.saturating_sub(first.rollup.vcsw), wall_s);
    out.ctx_per_s.invol = per_second(last.rollup.nvcsw.saturating_sub(first.rollup.nvcsw), wall_s);
    out.write_bytes = last
        .rollup
        .write_bytes
        .saturating_sub(first.rollup.write_bytes);
    out.wchar = last.rollup.wchar.saturating_sub(first.rollup.wchar);
    out.syscw = last.rollup.syscw.saturating_sub(first.rollup.syscw);
    out.threads.max = slice
        .iter()
        .map(|tick| tick.rollup.threads)
        .max()
        .unwrap_or(0);
    out.threads.end = last.rollup.threads;
    out.threads.by_comm = last.rollup.thread_names.clone();
    out.arena_heaps = last.rollup.arena_heaps;
    out.fds = last.rollup.fds;
    out.inotify = InotifyMetrics {
        watches: last.rollup.inotify_watches,
        dup: last.rollup.inotify_dup,
    };
    out.spawns = map_u64_delta(&first.rollup.spawns, &last.rollup.spawns);
    for (prog, count) in &set.extra_spawns {
        *out.spawns.entry(prog.clone()).or_default() += *count;
    }
    let busy_us = last
        .rollup
        .proc_busy_us
        .saturating_sub(first.rollup.proc_busy_us);
    if wall_s > 0.0 {
        let outside_us = busy_us.saturating_sub(tree_us);
        out.outside_cpu_pct = us_to_s(outside_us) / wall_s * 100.0;
    }
    if let (Some(start), Some(end)) = (
        first.rollup.real_engine_cpu_us,
        last.rollup.real_engine_cpu_us,
    ) {
        let cpu_s = us_to_s(end.saturating_sub(start));
        out.real_engine_cpu_s = Some(cpu_s);
        if wall_s > 0.0 {
            out.real_engine_cpu_pct = Some(cpu_s / wall_s * 100.0);
        }
    }
    out.psi_cpu = psi_window(first.rollup.psi_cpu, last.rollup.psi_cpu);
    out.psi_mem = psi_window(first.rollup.psi_mem, last.rollup.psi_mem);
    out
}

fn cpu_delta(start: &Rollup, end: &Rollup) -> (u64, u64, u64) {
    if let (Some(start_us), Some(end_us)) = (start.cpu_cgroup_us, end.cpu_cgroup_us) {
        let user = match (start.cpu_cgroup_user_us, end.cpu_cgroup_user_us) {
            (Some(a), Some(b)) => b.saturating_sub(a),
            _ => 0,
        };
        let system = match (start.cpu_cgroup_system_us, end.cpu_cgroup_system_us) {
            (Some(a), Some(b)) => b.saturating_sub(a),
            _ => 0,
        };
        return (end_us.saturating_sub(start_us), user, system);
    }
    (
        end.cpu_proc_us.saturating_sub(start.cpu_proc_us),
        end.cpu_proc_user_us.saturating_sub(start.cpu_proc_user_us),
        end.cpu_proc_system_us
            .saturating_sub(start.cpu_proc_system_us),
    )
}

fn window_bounds(ticks: &[Tick], window: Window) -> Option<(usize, usize)> {
    if window.end_ns < window.start_ns {
        return None;
    }
    let first = ticks
        .iter()
        .position(|tick| tick.t_ns >= window.start_ns && tick.t_ns <= window.end_ns)?;
    let last = ticks
        .iter()
        .rposition(|tick| tick.t_ns >= window.start_ns && tick.t_ns <= window.end_ns)?;
    Some((first, last))
}

fn window_seconds(window: Window) -> f64 {
    window.end_ns.saturating_sub(window.start_ns) as f64 / 1e9
}

fn us_to_s(us: u64) -> f64 {
    us as f64 / 1e6
}

fn bytes_to_mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn kib_to_mib(kib: u64) -> f64 {
    kib as f64 / 1024.0
}

fn per_second(delta: u64, wall_s: f64) -> f64 {
    if wall_s <= 0.0 {
        0.0
    } else {
        delta as f64 / wall_s
    }
}

fn map_us_delta(
    start: &BTreeMap<String, u64>,
    end: &BTreeMap<String, u64>,
) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for (key, value) in end {
        let base = start.get(key).copied().unwrap_or(0);
        let delta = value.saturating_sub(base);
        if delta > 0 {
            out.insert(key.clone(), us_to_s(delta));
        }
    }
    out
}

fn map_u64_delta(
    start: &BTreeMap<String, u64>,
    end: &BTreeMap<String, u64>,
) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    for (key, value) in end {
        let delta = value.saturating_sub(start.get(key).copied().unwrap_or(0));
        if delta > 0 {
            out.insert(key.clone(), delta);
        }
    }
    out
}

fn map_rate(
    start: &BTreeMap<String, u64>,
    end: &BTreeMap<String, u64>,
    wall_s: f64,
) -> BTreeMap<String, f64> {
    map_u64_delta(start, end)
        .into_iter()
        .map(|(key, delta)| (key, per_second(delta, wall_s)))
        .collect()
}

fn level_total(
    ticks: &[Tick],
    total: impl Fn(&Tick) -> u64,
    by_group: impl Fn(&Tick) -> &BTreeMap<String, u64>,
) -> PssMib {
    if ticks.is_empty() {
        return PssMib::default();
    }
    let mut peak = 0u64;
    let mut sum = 0u64;
    let mut group_peak: BTreeMap<String, u64> = BTreeMap::new();
    for tick in ticks {
        let value = total(tick);
        peak = peak.max(value);
        sum = sum.saturating_add(value);
        for (group, kib) in by_group(tick) {
            let slot = group_peak.entry(group.clone()).or_default();
            *slot = (*slot).max(*kib);
        }
    }
    let end_tick = ticks.last().expect("ticks is non-empty");
    PssMib {
        peak: kib_to_mib(peak),
        mean: kib_to_mib(sum) / ticks.len() as f64,
        end: kib_to_mib(total(end_tick)),
        by_group_peak: group_peak
            .iter()
            .map(|(group, kib)| (group.clone(), kib_to_mib(*kib)))
            .collect(),
        by_group_end: by_group(end_tick)
            .iter()
            .map(|(group, kib)| (group.clone(), kib_to_mib(*kib)))
            .collect(),
    }
}

fn level_parts(
    ticks: &[Tick],
    total: impl Fn(&Tick) -> u64,
    by_group: impl Fn(&Tick) -> &BTreeMap<String, u64>,
) -> MemLevel {
    let full = level_total(ticks, total, by_group);
    MemLevel {
        peak: full.peak,
        mean: full.mean,
        end: full.end,
        by_group_peak: full.by_group_peak,
    }
}

fn cg_anon(ticks: &[Tick]) -> Option<CgAnon> {
    let values: Vec<u64> = ticks
        .iter()
        .filter_map(|tick| tick.rollup.cg_anon)
        .collect();
    if values.is_empty() {
        return None;
    }
    let peak = values.iter().copied().max().unwrap_or(0);
    let end = values.last().copied().unwrap_or(0);
    let sum: u64 = values.iter().copied().sum();
    Some(CgAnon {
        peak: bytes_to_mib(peak),
        mean: bytes_to_mib(sum) / values.len() as f64,
        end: bytes_to_mib(end),
    })
}

fn psi_window(start: Option<Pressure>, end: Option<Pressure>) -> Option<PsiWindow> {
    let end = end?;
    let start = start.unwrap_or(Pressure {
        some_avg10: 0.0,
        some_total_us: 0,
        full_avg10: 0.0,
        full_total_us: 0,
    });
    Some(PsiWindow {
        some_avg10: end.some_avg10,
        some_total_us: end.some_total_us.saturating_sub(start.some_total_us),
        full_avg10: end.full_avg10,
        full_total_us: end.full_total_us.saturating_sub(start.full_total_us),
    })
}

/// Starts the sampler. The returned handle is the only way to mark or finish it.
pub struct Sampler;

impl Sampler {
    /// Sample `root_pid` and its descendants at `hz` ticks per second.
    ///
    /// `scope` supplies cgroup-wide CPU, memory and pressure. `groups` labels
    /// each pid; it is copied onto the sampler thread.
    pub fn start(
        root_pid: u32,
        scope: Option<Cgroup>,
        hz: f64,
        groups: &Grouping,
    ) -> Result<Handle> {
        if root_pid <= 1 {
            bail!("refusing to sample pid {root_pid}");
        }
        validate_hz(hz)?;
        if !Path::new(&format!("/proc/{root_pid}")).is_dir() {
            bail!("pid {root_pid} is not alive");
        }
        let (tx, rx) = mpsc::channel();
        let wake = eventfd_new().context("eventfd")?;
        let thread_wake = dup_fd(wake.as_raw_fd()).context("dup eventfd")?;
        let tid = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let tid_thread = std::sync::Arc::clone(&tid);
        let groups = groups.clone();
        let join = thread::Builder::new()
            .name("zb-sample".into())
            .spawn(move || {
                let id = unsafe { libc::syscall(libc::SYS_gettid) } as u32;
                tid_thread.store(id, std::sync::atomic::Ordering::Release);
                run_sampler(root_pid, scope, hz, groups, rx, thread_wake)
            })
            .context("spawn sampler thread")?;
        Ok(Handle {
            tx,
            wake,
            tid,
            join: Some(join),
        })
    }
}

/// Control handle for a running [`Sampler`].
///
/// Dropping it stops the thread. It does not signal the sampled processes.
pub struct Handle {
    tx: Sender<Cmd>,
    wake: OwnedFd,
    tid: std::sync::Arc<std::sync::atomic::AtomicU32>,
    join: Option<JoinHandle<SampleSet>>,
}

impl Handle {
    /// Take a sample now and record it as the start or end of the window.
    ///
    /// Returns after that sample has been stored, so a `/proc` read done by
    /// the caller afterwards is not earlier than the mark.
    pub fn mark(&self, edge: Mark) -> Result<()> {
        let (ack_tx, ack_rx) = mpsc::channel();
        self.tx
            .send(Cmd::Mark { edge, ack: ack_tx })
            .context("sampler stopped")?;
        wake(&self.wake)?;
        ack_rx.recv().context("sampler stopped before the mark")?;
        Ok(())
    }

    /// Change the tick rate. Used to drop from 4 Hz to 1 Hz for an idle window.
    pub fn set_hz(&self, hz: f64) -> Result<()> {
        validate_hz(hz)?;
        let (ack_tx, ack_rx) = mpsc::channel();
        self.tx
            .send(Cmd::SetHz { hz, ack: ack_tx })
            .context("sampler stopped")?;
        wake(&self.wake)?;
        ack_rx
            .recv()
            .context("sampler stopped before the rate change")?;
        Ok(())
    }

    /// Tid of the sampler thread, once it has started.
    pub fn sampler_tid(&self) -> Option<u32> {
        let tid = self.tid.load(std::sync::atomic::Ordering::Acquire);
        (tid > 0).then_some(tid)
    }

    /// Stop the thread and return every tick it took.
    pub fn finish(mut self) -> Result<SampleSet> {
        let _ = self.tx.send(Cmd::Stop);
        let _ = wake(&self.wake);
        let join = self.join.take().context("sampler already finished")?;
        join.join()
            .map_err(|_| anyhow::anyhow!("sampler thread panicked"))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if self.join.is_none() {
            return;
        }
        let _ = self.tx.send(Cmd::Stop);
        let _ = wake(&self.wake);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

enum Cmd {
    Mark { edge: Mark, ack: Sender<()> },
    SetHz { hz: f64, ack: Sender<()> },
    Stop,
}

fn validate_hz(hz: f64) -> Result<()> {
    if hz.is_finite() && hz > 0.0 && hz <= 1000.0 {
        Ok(())
    } else {
        bail!("hz must be in (0, 1000], got {hz}");
    }
}

fn run_sampler(
    root_pid: u32,
    cgroup: Option<Cgroup>,
    mut hz: f64,
    groups: Grouping,
    rx: Receiver<Cmd>,
    wake_fd: OwnedFd,
) -> SampleSet {
    let clk_tck = clk_tck();
    let containment = if cgroup.is_some() { "scope" } else { "pgroup" };
    let real_engine = crate::detect_real_engine().map(|engine| engine.pid);
    let mut period_ns = hz_to_ns(hz);
    let mut next = crate::mono_now().saturating_add(period_ns);
    let mut open: HashMap<u32, Watched> = HashMap::new();
    let mut exited = Exited::default();
    let mut wake_by_comm: BTreeMap<String, u64> = BTreeMap::new();
    let mut spawns: BTreeMap<String, u64> = BTreeMap::new();
    let mut arenas = 0u32;
    let mut ticks = Vec::new();
    let mut marks = Vec::new();
    let mut buf = Vec::with_capacity(8192);
    loop {
        let now = crate::mono_now();
        poll_wake(wake_fd.as_raw_fd(), timeout_ms(next, now));
        drain_event(wake_fd.as_raw_fd());
        let cmds = drain_cmds(&rx);
        let periodic = crate::mono_now() >= next;
        let stop = cmds.iter().any(|cmd| matches!(cmd, Cmd::Stop));
        let mark = cmds.iter().any(|cmd| matches!(cmd, Cmd::Mark { .. }));
        for cmd in &cmds {
            if let Cmd::SetHz { hz: new_hz, ack } = cmd {
                hz = *new_hz;
                period_ns = hz_to_ns(hz);
                next = crate::mono_now().saturating_add(period_ns);
                let _ = ack.send(());
            }
        }
        if mark
            && cmds.iter().any(|cmd| {
                matches!(
                    cmd,
                    Cmd::Mark {
                        edge: Mark::WindowStart,
                        ..
                    }
                )
            })
        {
            if let Some(scope) = &cgroup {
                let _ = scope.reset_memory_peak();
            }
        }
        if periodic || mark || stop || ticks.is_empty() {
            ticks.push(take_sample(
                root_pid,
                cgroup.as_ref(),
                &groups,
                &mut open,
                &mut exited,
                &mut wake_by_comm,
                &mut spawns,
                &mut arenas,
                &mut buf,
                clk_tck,
                real_engine,
            ));
            let t_ns = ticks.last().map(|tick| tick.t_ns).unwrap_or(0);
            for cmd in &cmds {
                if let Cmd::Mark { edge, ack } = cmd {
                    marks.push(Marked { kind: *edge, t_ns });
                    let _ = ack.send(());
                }
            }
        }
        if stop {
            break;
        }
        if periodic {
            next = next.saturating_add(period_ns);
            let now = crate::mono_now();
            if next < now {
                next = now.saturating_add(period_ns);
            }
        }
    }
    SampleSet {
        root_pid,
        hz,
        containment,
        clk_tck,
        ticks,
        marks,
        data_dir_growth: DataDirGrowth::default(),
        extra_spawns: BTreeMap::new(),
        rpc_bytes_in: BTreeMap::new(),
        rpc_items: BTreeMap::new(),
    }
}

fn add_u64(map: &mut BTreeMap<String, u64>, key: &str, add: u64) {
    if let Some(slot) = map.get_mut(key) {
        *slot = slot.saturating_add(add);
    } else {
        map.insert(key.to_owned(), add);
    }
}

fn add_u32(map: &mut BTreeMap<String, u32>, key: &str, add: u32) {
    if let Some(slot) = map.get_mut(key) {
        *slot = slot.saturating_add(add);
    } else {
        map.insert(key.to_owned(), add);
    }
}

#[allow(clippy::too_many_arguments)]
fn take_sample(
    root_pid: u32,
    cgroup: Option<&Cgroup>,
    groups: &Grouping,
    open: &mut HashMap<u32, Watched>,
    exited: &mut Exited,
    wake_by_comm: &mut BTreeMap<String, u64>,
    spawns: &mut BTreeMap<String, u64>,
    arenas: &mut u32,
    buf: &mut Vec<u8>,
    clk_tck: u64,
    real_engine: Option<u32>,
) -> Tick {
    let snap = cgroup.map(Cgroup::snapshot).unwrap_or_default();
    discover(root_pid, groups, open, &snap, spawns, buf);
    let pids: Vec<u32> = open.keys().copied().collect();
    let mut dead = Vec::new();
    for pid in pids {
        let Some(watched) = open.get_mut(&pid) else {
            continue;
        };
        if !watched.refresh(buf, wake_by_comm) {
            dead.push(pid);
        }
    }
    for pid in dead {
        if let Some(watched) = open.remove(&pid) {
            exited.fold(pid, root_pid, watched);
        }
    }
    let mut procs = Vec::with_capacity(open.len());
    let mut live_user = 0u64;
    let mut live_sys = 0u64;
    let mut by_group_ticks: BTreeMap<String, u64> = BTreeMap::new();
    let mut pss_kib = 0u64;
    let mut pss_anon_kib = 0u64;
    let mut pss_file_kib = 0u64;
    let mut swap_pss_kib = 0u64;
    let mut pss_by_group: BTreeMap<String, u64> = BTreeMap::new();
    let mut anon_by_group: BTreeMap<String, u64> = BTreeMap::new();
    let mut file_by_group: BTreeMap<String, u64> = BTreeMap::new();
    let mut swap_by_group: BTreeMap<String, u64> = BTreeMap::new();
    let mut sched = exited.sched;
    let mut vcsw = exited.vcsw;
    let mut nvcsw = exited.nvcsw;
    let mut write_bytes = exited.write_bytes;
    let mut wchar = exited.wchar;
    let mut syscw = exited.syscw;
    let mut threads = 0u32;
    let mut thread_names: BTreeMap<String, u32> = BTreeMap::new();
    let mut watches = Vec::new();
    let mut fds = 0u32;
    let mut rss_bytes = 0u64;
    for (pid, watched) in open.iter() {
        procs.push(watched.snap(*pid, clk_tck));
        live_user = live_user.saturating_add(watched.utime);
        live_sys = live_sys.saturating_add(watched.stime);
        add_u64(
            &mut by_group_ticks,
            &watched.group,
            watched.utime.saturating_add(watched.stime),
        );
        pss_kib = pss_kib.saturating_add(watched.pss_kib);
        pss_anon_kib = pss_anon_kib.saturating_add(watched.pss_anon_kib);
        pss_file_kib = pss_file_kib.saturating_add(watched.pss_file_kib);
        swap_pss_kib = swap_pss_kib.saturating_add(watched.swap_pss_kib);
        add_u64(&mut pss_by_group, &watched.group, watched.pss_kib);
        add_u64(&mut anon_by_group, &watched.group, watched.pss_anon_kib);
        add_u64(&mut file_by_group, &watched.group, watched.pss_file_kib);
        add_u64(&mut swap_by_group, &watched.group, watched.swap_pss_kib);
        sched = sched.saturating_add(watched.sched_total());
        vcsw = vcsw.saturating_add(watched.vcsw_total());
        nvcsw = nvcsw.saturating_add(watched.nvcsw_total());
        write_bytes = write_bytes.saturating_add(watched.write_bytes);
        wchar = wchar.saturating_add(watched.wchar);
        syscw = syscw.saturating_add(watched.syscw);
        threads = threads.saturating_add(watched.threads.max(1));
        if watched.thread_names.is_empty() {
            add_u32(&mut thread_names, &watched.comm, watched.threads.max(1));
        } else {
            for (name, count) in &watched.thread_names {
                add_u32(&mut thread_names, name, *count);
            }
        }
        rss_bytes = rss_bytes.saturating_add(watched.rss_kib.saturating_mul(1024));
        if let Some(fd_dir) = &watched.files.fd_dir {
            let scan: FdScan = scan_fds(fd_dir.as_raw_fd());
            fds = fds.saturating_add(scan.count);
            watches.extend(scan.watches);
        }
    }
    for (group, ticks) in &exited.by_group_ticks {
        add_u64(&mut by_group_ticks, group, *ticks);
    }
    procs.sort_by_key(|proc| proc.pid);
    let engine_ticks = engine_ticks(root_pid, open, exited);
    let engine_cut = engine_cutime(root_pid, open, exited);
    let inotify = summarize_inotify(watches);
    if let Some(root) = open.get(&root_pid) {
        if let Ok(maps) = open_at(root.files.dir.as_raw_fd(), c"maps") {
            if reread(maps.as_raw_fd(), buf).is_ok() {
                if let Some(text) = text_of(buf) {
                    *arenas = count_arena_heaps(text);
                }
            }
        }
    }
    let cpu_proc_user = ticks_to_us(live_user.saturating_add(engine_cut.0), clk_tck);
    let cpu_proc_system = ticks_to_us(live_sys.saturating_add(engine_cut.1), clk_tck);
    let cpu_proc = cpu_proc_user.saturating_add(cpu_proc_system);
    let cpu_by_group_us = by_group_ticks
        .iter()
        .map(|(group, ticks)| (group.clone(), ticks_to_us(*ticks, clk_tck)))
        .collect();
    let (cpu_cgroup_us, cpu_cgroup_user_us, cpu_cgroup_system_us) = match snap.cpu {
        Some(cpu) => (
            Some(cpu.usage_usec),
            Some(cpu.user_usec),
            Some(cpu.system_usec),
        ),
        None => (None, None, None),
    };
    let tree_cpu_us = cpu_cgroup_us.unwrap_or(cpu_proc);
    let tree_mem_cur = snap.mem_current.unwrap_or(rss_bytes);
    let tree_anon = snap.anon.unwrap_or(pss_anon_kib.saturating_mul(1024));
    let proc_busy_us = read_proc_busy_us(clk_tck);
    let real_engine_cpu_us = real_engine.and_then(|pid| read_pid_cpu_us(pid, clk_tck));
    Tick {
        t_ns: crate::mono_now(),
        tree_cpu_us,
        tree_mem_cur,
        tree_anon,
        tree_pids: u32::try_from(procs.len()).unwrap_or(u32::MAX),
        procs,
        rollup: Rollup {
            cpu_cgroup_us,
            cpu_cgroup_user_us,
            cpu_cgroup_system_us,
            cpu_proc_us: cpu_proc,
            cpu_proc_user_us: cpu_proc_user,
            cpu_proc_system_us: cpu_proc_system,
            cpu_engine_us: ticks_to_us(engine_ticks, clk_tck),
            cpu_by_group_us,
            pss_kib,
            pss_anon_kib,
            pss_file_kib,
            swap_pss_kib,
            pss_by_group_kib: pss_by_group,
            pss_anon_by_group_kib: anon_by_group,
            pss_file_by_group_kib: file_by_group,
            swap_pss_by_group_kib: swap_by_group,
            sched_pcount: sched,
            wake_by_comm: wake_by_comm.clone(),
            vcsw,
            nvcsw,
            write_bytes,
            wchar,
            syscw,
            threads,
            thread_names,
            arena_heaps: *arenas,
            fds,
            inotify_watches: inotify.watches,
            inotify_dup: inotify.dup,
            spawns: spawns.clone(),
            cg_mem_current: snap.mem_current,
            cg_mem_peak: snap.mem_peak,
            cg_anon: snap.anon,
            psi_cpu: snap.cpu_pressure,
            psi_mem: snap.mem_pressure,
            proc_busy_us,
            real_engine_cpu_us,
        },
    }
}

fn engine_ticks(root: u32, open: &HashMap<u32, Watched>, exited: &Exited) -> u64 {
    if let Some(root) = open.get(&root) {
        root.utime.saturating_add(root.stime)
    } else {
        exited.engine_utime.saturating_add(exited.engine_stime)
    }
}

fn engine_cutime(root: u32, open: &HashMap<u32, Watched>, exited: &Exited) -> (u64, u64) {
    if let Some(root) = open.get(&root) {
        (root.cutime, root.cstime)
    } else {
        (exited.engine_cutime, exited.engine_cstime)
    }
}

fn discover(
    root: u32,
    groups: &Grouping,
    open: &mut HashMap<u32, Watched>,
    snap: &CgroupSnap,
    spawns: &mut BTreeMap<String, u64>,
    buf: &mut Vec<u8>,
) {
    let mut stack = vec![root];
    stack.extend(snap.procs.iter().copied());
    let mut seen = HashSet::new();
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if pid <= 1 && pid != root {
            continue;
        }
        if open.len() > 8192 {
            break;
        }
        if !open.contains_key(&pid) {
            let Some(watched) = Watched::open(pid, root, groups) else {
                continue;
            };
            *spawns.entry(watched.prog.clone()).or_default() += 1;
            open.insert(pid, watched);
        }
        let children = open
            .get(&pid)
            .map(|watched| watched.read_children(buf))
            .unwrap_or_default();
        stack.extend(children);
    }
}

struct PidFiles {
    dir: OwnedFd,
    smaps: Option<OwnedFd>,
    stat: OwnedFd,
    io: Option<OwnedFd>,
    status: Option<OwnedFd>,
    sched: Option<OwnedFd>,
    children: Option<OwnedFd>,
    fd_dir: Option<OwnedFd>,
}

struct TaskLast {
    sched: u64,
    vcsw: u64,
    nvcsw: u64,
    ready: bool,
}

struct Watched {
    pid: u32,
    files: PidFiles,
    group: String,
    prog: String,
    comm: String,
    ppid: u32,
    starttime: u64,
    utime: u64,
    stime: u64,
    cutime: u64,
    cstime: u64,
    wchar: u64,
    write_bytes: u64,
    syscw: u64,
    rss_kib: u64,
    pss_kib: u64,
    pss_anon_kib: u64,
    pss_file_kib: u64,
    swap_pss_kib: u64,
    threads: u32,
    /// Process-level timeslices. Used only while the pid stays single-threaded.
    /// That counter is the thread-group leader, not the sum of every task.
    sched: u64,
    vcsw: u64,
    nvcsw: u64,
    sched_ready: bool,
    tasks: HashMap<u32, TaskLast>,
    thread_names: BTreeMap<String, u32>,
    exited_sched: u64,
    exited_vcsw: u64,
    exited_nvcsw: u64,
    /// Baselines of tasks first seen after the leader. Subtracted so a thread's
    /// history from before we started walking `task/` does not look like a
    /// burst of wakeups on the sample that discovered it.
    sched_skip: u64,
    vcsw_skip: u64,
    nvcsw_skip: u64,
    multi: bool,
}

impl Watched {
    fn open(pid: u32, root: u32, groups: &Grouping) -> Option<Self> {
        let dir = open_read(Path::new(&format!("/proc/{pid}"))).ok()?;
        let stat = open_at(dir.as_raw_fd(), c"stat").ok()?;
        let (exe, cmdline) = read_identity(pid);
        let group = groups.classify(root, pid, &exe, &cmdline);
        let prog = prog_name(&exe, &cmdline);
        Some(Self {
            pid,
            files: PidFiles {
                smaps: open_at(dir.as_raw_fd(), c"smaps_rollup").ok(),
                io: open_at(dir.as_raw_fd(), c"io").ok(),
                status: open_at(dir.as_raw_fd(), c"status").ok(),
                sched: open_at(dir.as_raw_fd(), c"schedstat").ok(),
                children: open_read(Path::new(&format!("/proc/{pid}/task/{pid}/children"))).ok(),
                fd_dir: open_read(Path::new(&format!("/proc/{pid}/fd"))).ok(),
                dir,
                stat,
            },
            group,
            prog,
            comm: String::new(),
            ppid: 0,
            starttime: 0,
            utime: 0,
            stime: 0,
            cutime: 0,
            cstime: 0,
            wchar: 0,
            write_bytes: 0,
            syscw: 0,
            rss_kib: 0,
            pss_kib: 0,
            pss_anon_kib: 0,
            pss_file_kib: 0,
            swap_pss_kib: 0,
            threads: 1,
            sched: 0,
            vcsw: 0,
            nvcsw: 0,
            sched_ready: false,
            tasks: HashMap::new(),
            thread_names: BTreeMap::new(),
            exited_sched: 0,
            exited_vcsw: 0,
            exited_nvcsw: 0,
            sched_skip: 0,
            vcsw_skip: 0,
            nvcsw_skip: 0,
            multi: false,
        })
    }

    fn refresh(&mut self, buf: &mut Vec<u8>, wake: &mut BTreeMap<String, u64>) -> bool {
        if reread(self.files.stat.as_raw_fd(), buf).is_err() {
            return false;
        }
        let parsed = {
            let Some(stat) = parse_stat_view(buf) else {
                return false;
            };
            if self.starttime != 0 && stat.starttime != self.starttime {
                return false;
            }
            let comm = if self.comm.as_bytes() == stat.comm {
                None
            } else {
                Some(String::from_utf8_lossy(stat.comm).into_owned())
            };
            (
                comm,
                stat.ppid,
                stat.utime,
                stat.stime,
                stat.cutime,
                stat.cstime,
                stat.num_threads.max(1),
                stat.starttime,
            )
        };
        if let Some(comm) = parsed.0 {
            self.comm = comm;
        }
        self.ppid = parsed.1;
        self.utime = parsed.2;
        self.stime = parsed.3;
        self.cutime = parsed.4;
        self.cstime = parsed.5;
        self.threads = parsed.6;
        self.starttime = parsed.7;
        let num_threads = self.threads;
        if let Some(fd) = &self.files.smaps {
            if reread(fd.as_raw_fd(), buf).is_ok() {
                let smaps = parse_smaps_rollup_bytes(buf);
                self.rss_kib = smaps.rss_kib;
                self.pss_kib = smaps.pss_kib;
                self.pss_anon_kib = smaps.pss_anon_kib;
                self.pss_file_kib = smaps.pss_file_kib;
                self.swap_pss_kib = smaps.swap_pss_kib;
            }
        }
        if let Some(fd) = &self.files.io {
            if reread(fd.as_raw_fd(), buf).is_ok() {
                let io = parse_io_bytes(buf);
                self.wchar = io.wchar;
                self.write_bytes = io.write_bytes;
                self.syscw = io.syscw;
            }
        }
        if num_threads <= 1 {
            self.collapse_threads();
            self.refresh_leader(buf, wake);
        } else {
            self.refresh_tasks(buf, wake);
        }
        true
    }

    fn collapse_threads(&mut self) {
        if !self.multi && self.tasks.is_empty() {
            return;
        }
        let leader = self.pid;
        let gone: Vec<u32> = self.tasks.keys().copied().collect();
        let mut kept_leader = false;
        for tid in gone {
            let Some(task) = self.tasks.remove(&tid) else {
                continue;
            };
            if tid == leader {
                // Process schedstat is this same counter. Folding it as well
                // would count the leader twice on the next single-thread sample.
                self.sched = task.sched;
                self.vcsw = task.vcsw;
                self.nvcsw = task.nvcsw;
                self.sched_ready = task.ready;
                kept_leader = true;
                continue;
            }
            self.exited_sched = self.exited_sched.saturating_add(task.sched);
            self.exited_vcsw = self.exited_vcsw.saturating_add(task.vcsw);
            self.exited_nvcsw = self.exited_nvcsw.saturating_add(task.nvcsw);
        }
        self.tasks.clear();
        self.multi = false;
        if !kept_leader {
            self.sched_ready = false;
        }
    }

    fn refresh_leader(&mut self, buf: &mut Vec<u8>, wake: &mut BTreeMap<String, u64>) {
        if let Some(fd) = &self.files.sched {
            if reread(fd.as_raw_fd(), buf).is_ok()
                && let Some(sched) = parse_schedstat_bytes(buf)
            {
                if self.sched_ready {
                    let delta = sched.timeslices.saturating_sub(self.sched);
                    if delta > 0 {
                        *wake.entry(self.comm.clone()).or_default() += delta;
                    }
                }
                self.sched = sched.timeslices;
                self.sched_ready = true;
            }
        }
        if let Some(fd) = &self.files.status {
            if reread(fd.as_raw_fd(), buf).is_ok() {
                let switches = parse_status_switches_bytes(buf);
                self.vcsw = switches.voluntary;
                self.nvcsw = switches.nonvoluntary;
            }
        }
        let count = self.threads.max(1);
        let unchanged = self.thread_names.len() == 1
            && self.thread_names.get(&self.comm).copied() == Some(count);
        if !unchanged {
            self.thread_names.clear();
            self.thread_names.insert(self.comm.clone(), count);
        }
    }

    fn refresh_tasks(&mut self, buf: &mut Vec<u8>, wake: &mut BTreeMap<String, u64>) {
        self.multi = true;
        let task_dir = format!("/proc/{}/task", self.pid);
        let Ok(entries) = fs::read_dir(&task_dir) else {
            if self.thread_names.is_empty() {
                self.thread_names
                    .insert(self.comm.clone(), self.threads.max(1));
            }
            return;
        };
        self.thread_names.clear();
        let mut live = HashSet::new();
        for entry in entries.flatten() {
            let Ok(tid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            if tid == 0 {
                continue;
            }
            let Some(sched) = read_task_sched(self.pid, tid, buf) else {
                continue;
            };
            let switches = read_task_switches(self.pid, tid, buf).unwrap_or_default();
            let comm = read_task_comm(self.pid, tid, buf).unwrap_or_default();
            live.insert(tid);
            self.observe_task(
                tid,
                &comm,
                sched.timeslices,
                switches.voluntary,
                switches.nonvoluntary,
                wake,
            );
        }
        if live.is_empty() {
            if self.thread_names.is_empty() {
                self.thread_names
                    .insert(self.comm.clone(), self.threads.max(1));
            }
            return;
        }
        let gone: Vec<u32> = self
            .tasks
            .keys()
            .copied()
            .filter(|tid| !live.contains(tid))
            .collect();
        for tid in gone {
            let Some(task) = self.tasks.remove(&tid) else {
                continue;
            };
            self.exited_sched = self.exited_sched.saturating_add(task.sched);
            self.exited_vcsw = self.exited_vcsw.saturating_add(task.vcsw);
            self.exited_nvcsw = self.exited_nvcsw.saturating_add(task.nvcsw);
        }
    }

    fn observe_task(
        &mut self,
        tid: u32,
        comm: &str,
        sched: u64,
        vcsw: u64,
        nvcsw: u64,
        wake: &mut BTreeMap<String, u64>,
    ) {
        let name = if comm.is_empty() {
            self.comm.clone()
        } else {
            comm.to_string()
        };
        *self.thread_names.entry(name.clone()).or_default() += 1;
        if let Some(task) = self.tasks.get_mut(&tid) {
            if task.ready {
                let delta = sched.saturating_sub(task.sched);
                if delta > 0 {
                    *wake.entry(name).or_default() += delta;
                }
            }
            task.sched = sched;
            task.vcsw = vcsw;
            task.nvcsw = nvcsw;
            task.ready = true;
            return;
        }
        if tid == self.pid && self.sched_ready {
            let delta = sched.saturating_sub(self.sched);
            if delta > 0 {
                *wake.entry(name).or_default() += delta;
            }
        } else if tid != self.pid {
            self.sched_skip = self.sched_skip.saturating_add(sched);
            self.vcsw_skip = self.vcsw_skip.saturating_add(vcsw);
            self.nvcsw_skip = self.nvcsw_skip.saturating_add(nvcsw);
        }
        self.tasks.insert(
            tid,
            TaskLast {
                sched,
                vcsw,
                nvcsw,
                ready: true,
            },
        );
    }

    fn read_children(&self, buf: &mut Vec<u8>) -> Vec<u32> {
        let Some(fd) = &self.files.children else {
            return Vec::new();
        };
        if reread(fd.as_raw_fd(), buf).is_err() {
            return Vec::new();
        }
        text_of(buf).map(parse_children).unwrap_or_default()
    }

    fn sched_total(&self) -> u64 {
        let raw = if self.multi {
            self.tasks.values().fold(self.exited_sched, |sum, task| {
                sum.saturating_add(task.sched)
            })
        } else {
            self.exited_sched.saturating_add(self.sched)
        };
        raw.saturating_sub(self.sched_skip)
    }

    fn vcsw_total(&self) -> u64 {
        let raw = if self.multi {
            self.tasks
                .values()
                .fold(self.exited_vcsw, |sum, task| sum.saturating_add(task.vcsw))
        } else {
            self.exited_vcsw.saturating_add(self.vcsw)
        };
        raw.saturating_sub(self.vcsw_skip)
    }

    fn nvcsw_total(&self) -> u64 {
        let raw = if self.multi {
            self.tasks.values().fold(self.exited_nvcsw, |sum, task| {
                sum.saturating_add(task.nvcsw)
            })
        } else {
            self.exited_nvcsw.saturating_add(self.nvcsw)
        };
        raw.saturating_sub(self.nvcsw_skip)
    }

    fn snap(&self, pid: u32, clk_tck: u64) -> ProcSnap {
        ProcSnap {
            pid,
            ppid: self.ppid,
            group: self.group.clone(),
            comm: self.comm.clone(),
            pss: self.pss_kib.saturating_mul(1024),
            pss_anon: self.pss_anon_kib.saturating_mul(1024),
            pss_file: self.pss_file_kib.saturating_mul(1024),
            swap_pss: self.swap_pss_kib.saturating_mul(1024),
            rss: self.rss_kib.saturating_mul(1024),
            utime: ticks_to_us(self.utime, clk_tck),
            stime: ticks_to_us(self.stime, clk_tck),
            threads: self.threads,
            wb: self.write_bytes,
            syscw: self.syscw,
            sched_pcount: self.sched_total(),
            vcsw: self.vcsw_total(),
            nvcsw: self.nvcsw_total(),
        }
    }
}

fn read_task_sched(pid: u32, tid: u32, buf: &mut Vec<u8>) -> Option<Schedstat> {
    let path = format!("/proc/{pid}/task/{tid}/schedstat");
    let fd = open_read(Path::new(&path)).ok()?;
    reread(fd.as_raw_fd(), buf).ok()?;
    parse_schedstat_bytes(buf)
}

fn read_task_switches(pid: u32, tid: u32, buf: &mut Vec<u8>) -> Option<StatusSwitches> {
    let path = format!("/proc/{pid}/task/{tid}/status");
    let fd = open_read(Path::new(&path)).ok()?;
    read_parsed(fd.as_raw_fd(), buf, |text| {
        Some(parse_status_switches_bytes(text.as_bytes()))
    })
}

fn read_task_comm(pid: u32, tid: u32, buf: &mut Vec<u8>) -> Option<String> {
    let path = format!("/proc/{pid}/task/{tid}/comm");
    let fd = open_read(Path::new(&path)).ok()?;
    reread(fd.as_raw_fd(), buf).ok()?;
    Some(text_of(buf)?.trim().to_string())
}

#[derive(Default)]
struct Exited {
    by_group_ticks: BTreeMap<String, u64>,
    wchar: u64,
    write_bytes: u64,
    syscw: u64,
    sched: u64,
    vcsw: u64,
    nvcsw: u64,
    engine_utime: u64,
    engine_stime: u64,
    engine_cutime: u64,
    engine_cstime: u64,
}

impl Exited {
    fn fold(&mut self, pid: u32, root: u32, watched: Watched) {
        let own = watched.utime.saturating_add(watched.stime);
        *self
            .by_group_ticks
            .entry(watched.group.clone())
            .or_default() += own;
        self.wchar = self.wchar.saturating_add(watched.wchar);
        self.write_bytes = self.write_bytes.saturating_add(watched.write_bytes);
        self.syscw = self.syscw.saturating_add(watched.syscw);
        self.sched = self.sched.saturating_add(watched.sched_total());
        self.vcsw = self.vcsw.saturating_add(watched.vcsw_total());
        self.nvcsw = self.nvcsw.saturating_add(watched.nvcsw_total());
        if pid == root {
            self.engine_utime = watched.utime;
            self.engine_stime = watched.stime;
            self.engine_cutime = watched.cutime;
            self.engine_cstime = watched.cstime;
        }
    }
}

fn read_parsed<T>(
    fd: std::os::fd::RawFd,
    buf: &mut Vec<u8>,
    parse: impl FnOnce(&str) -> Option<T>,
) -> Option<T> {
    reread(fd, buf).ok()?;
    let text = text_of(buf)?;
    parse(text)
}

fn read_identity(pid: u32) -> (String, String) {
    let exe = fs::read_link(format!("/proc/{pid}/exe"))
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let raw = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let cmdline = String::from_utf8_lossy(&raw).replace('\0', " ");
    (exe, cmdline.trim().to_string())
}

fn read_proc_busy_us(clk_tck: u64) -> u64 {
    let Ok(text) = fs::read_to_string("/proc/stat") else {
        return 0;
    };
    let Some(cpu) = parse_machine_cpu(&text) else {
        return 0;
    };
    ticks_to_us(cpu.busy_jiffies, clk_tck)
}

fn read_pid_cpu_us(pid: u32, clk_tck: u64) -> Option<u64> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let stat = parse_stat(&text)?;
    Some(ticks_to_us(stat.utime.saturating_add(stat.stime), clk_tck))
}

fn ticks_to_us(ticks: u64, clk_tck: u64) -> u64 {
    if clk_tck == 0 {
        return 0;
    }
    ticks.saturating_mul(1_000_000) / clk_tck
}

fn clk_tck() -> u64 {
    let value = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if value > 0 { value as u64 } else { 100 }
}

fn hz_to_ns(hz: f64) -> u64 {
    let ns = (1e9 / hz).round();
    if ns.is_finite() && ns > 0.0 {
        ns as u64
    } else {
        1_000_000_000
    }
}

fn timeout_ms(next: u64, now: u64) -> i32 {
    if now >= next {
        return 0;
    }
    let ms = (next - now).div_ceil(1_000_000);
    i32::try_from(ms).unwrap_or(i32::MAX)
}

fn poll_wake(fd: std::os::fd::RawFd, timeout_ms: i32) {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: `pfd` is a valid pollfd. `poll` only writes `revents`.
        let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
        }
        break;
    }
}

fn drain_event(fd: std::os::fd::RawFd) {
    let mut buf = [0u8; 8];
    loop {
        // SAFETY: `buf` is 8 bytes, the eventfd value size.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if n == 0 {
            break;
        }
    }
}

fn drain_cmds(rx: &Receiver<Cmd>) -> Vec<Cmd> {
    let mut cmds = Vec::new();
    while let Ok(cmd) = rx.try_recv() {
        cmds.push(cmd);
    }
    cmds
}

fn eventfd_new() -> io::Result<OwnedFd> {
    // SAFETY: `eventfd` returns a new fd or -1. No other state is touched.
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn dup_fd(fd: std::os::fd::RawFd) -> io::Result<OwnedFd> {
    // SAFETY: `fd` is open. `F_DUPFD_CLOEXEC` returns a new fd or -1.
    let duped = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duped < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(duped) })
}

fn wake(fd: &OwnedFd) -> io::Result<()> {
    let one = 1u64;
    loop {
        // SAFETY: `one` is the 8-byte eventfd token. `fd` is an eventfd.
        let n = unsafe { libc::write(fd.as_raw_fd(), (&one as *const u64).cast(), 8) };
        if n == 8 {
            return Ok(());
        }
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            // Already pending. The sampler will wake.
            if err.raw_os_error() == Some(libc::EAGAIN) {
                return Ok(());
            }
            return Err(err);
        }
    }
}

fn basename(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
}

fn first_token(cmdline: &str) -> &str {
    cmdline.split_whitespace().next().unwrap_or("")
}

fn prog_name(exe: &str, cmdline: &str) -> String {
    let exe = exe.strip_suffix(" (deleted)").unwrap_or(exe);
    let base = basename(exe);
    if !base.is_empty() {
        return base.to_string();
    }
    basename(first_token(cmdline)).to_string()
}

fn is_npm_wrapper(base: &str, cmdline: &str) -> bool {
    if base == "npx" {
        return true;
    }
    let tokens: Vec<&str> = cmdline.split_whitespace().collect();
    if base == "npm" && tokens.iter().any(|token| *token == "exec") {
        return true;
    }
    matches!(base, "sh" | "bash" | "dash") && tokens.iter().any(|token| *token == "-c")
}

fn is_browser(base: &str) -> bool {
    let base = base.to_ascii_lowercase();
    base == "chrome"
        || base == "chromium"
        || base.starts_with("google-chrome")
        || base.contains("chromium")
        || base.starts_with("chrome")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rollup_with(cpu_proc_us: u64, pss_kib: u64) -> Rollup {
        Rollup {
            cpu_proc_us,
            cpu_proc_user_us: cpu_proc_us,
            cpu_engine_us: cpu_proc_us,
            pss_kib,
            pss_by_group_kib: BTreeMap::from([("engine".into(), pss_kib)]),
            ..Rollup::default()
        }
    }

    fn tick(t_ns: u64, rollup: Rollup) -> Tick {
        let tree_cpu_us = rollup.cpu_cgroup_us.unwrap_or(rollup.cpu_proc_us);
        Tick {
            t_ns,
            tree_cpu_us,
            tree_mem_cur: rollup.cg_mem_current.unwrap_or(0),
            tree_anon: rollup.cg_anon.unwrap_or(0),
            tree_pids: 1,
            procs: Vec::new(),
            rollup,
        }
    }

    #[test]
    fn grouping_first_match_wins() {
        let groups = Grouping::default();
        assert_eq!(
            groups.classify(10, 10, "/usr/bin/cargo", "cargo build"),
            "engine"
        );
        assert_eq!(
            groups.classify(1, 2, "/opt/zb-replay", "zb-replay --print"),
            "replay"
        );
        assert_eq!(groups.classify(1, 2, "/usr/bin/claude", "claude"), "claude");
        assert_eq!(
            groups.classify(1, 2, "/usr/bin/node", "node chrome-devtools-mcp"),
            "mcp:chrome-devtools"
        );
        assert_eq!(
            groups.classify(1, 2, "/usr/bin/npm", "npm exec @playwright/mcp"),
            "mcp:playwright"
        );
        assert_eq!(
            groups.classify(1, 2, "/usr/bin/node", "node /usr/bin/zeron mcp"),
            "mcp:zeron"
        );
        assert_eq!(
            groups.classify(1, 2, "/usr/bin/npx", "npx cowsay"),
            "npm-wrapper"
        );
        assert_eq!(
            groups.classify(1, 2, "/bin/sh", "sh -c npm exec foo"),
            "npm-wrapper"
        );
        assert_eq!(
            groups.classify(1, 2, "/usr/bin/npm", "npm install"),
            "other"
        );
        assert_eq!(
            groups.classify(1, 2, "/opt/google-chrome", "chrome"),
            "browser"
        );
        assert_eq!(groups.classify(1, 2, "/usr/bin/git", "git status"), "git");
        assert_eq!(
            groups.classify(1, 2, "/usr/bin/rustc", "rustc x.rs"),
            "build"
        );
        assert_eq!(groups.classify(1, 2, "/usr/bin/cc", "cc -c x.c"), "build");
        assert_eq!(groups.classify(1, 2, "/usr/bin/ld", "ld"), "build");
        assert_eq!(groups.classify(1, 2, "/usr/bin/sleep", "sleep 1"), "other");
        let custom = Grouping {
            claude: vec!["/usr/local/bin/claude-real".into()],
            mcp: Vec::new(),
        };
        assert_eq!(
            custom.classify(1, 2, "/usr/local/bin/claude-real", "claude-real"),
            "claude"
        );
        assert_eq!(custom.classify(1, 2, "/usr/bin/claude", "claude"), "other");
    }

    #[test]
    fn metrics_reports_rates_peaks_and_prefers_cgroup_cpu() {
        let start = Rollup {
            cpu_cgroup_us: Some(1_000_000),
            cpu_cgroup_user_us: Some(800_000),
            cpu_cgroup_system_us: Some(200_000),
            cpu_proc_us: 50_000,
            cpu_engine_us: 10_000,
            cpu_by_group_us: BTreeMap::from([("engine".into(), 10_000), ("git".into(), 1_000)]),
            pss_kib: 1024,
            pss_anon_kib: 512,
            pss_file_kib: 256,
            swap_pss_kib: 128,
            pss_by_group_kib: BTreeMap::from([("engine".into(), 1024)]),
            sched_pcount: 100,
            wake_by_comm: BTreeMap::from([("tokio-runtime-w".into(), 80)]),
            vcsw: 50,
            nvcsw: 5,
            write_bytes: 10,
            wchar: 20,
            syscw: 2,
            threads: 4,
            spawns: BTreeMap::from([("git".into(), 1)]),
            cg_mem_peak: Some(2 * 1024 * 1024),
            cg_anon: Some(1024 * 1024),
            psi_cpu: Some(Pressure {
                some_avg10: 0.0,
                some_total_us: 10,
                full_avg10: 0.0,
                full_total_us: 1,
            }),
            proc_busy_us: 5_000_000,
            real_engine_cpu_us: Some(100_000),
            ..Rollup::default()
        };
        let end = Rollup {
            cpu_cgroup_us: Some(3_000_000),
            cpu_cgroup_user_us: Some(2_400_000),
            cpu_cgroup_system_us: Some(600_000),
            cpu_proc_us: 90_000,
            cpu_engine_us: 40_000,
            cpu_by_group_us: BTreeMap::from([("engine".into(), 40_000), ("git".into(), 5_000)]),
            pss_kib: 3072,
            pss_anon_kib: 2048,
            pss_file_kib: 512,
            swap_pss_kib: 256,
            pss_by_group_kib: BTreeMap::from([("engine".into(), 2048), ("git".into(), 1024)]),
            sched_pcount: 300,
            wake_by_comm: BTreeMap::from([("tokio-runtime-w".into(), 280)]),
            vcsw: 250,
            nvcsw: 15,
            write_bytes: 110,
            wchar: 70,
            syscw: 9,
            threads: 9,
            thread_names: BTreeMap::from([("tokio-runtime-w".into(), 8), ("main".into(), 1)]),
            arena_heaps: 3,
            fds: 12,
            inotify_watches: 5,
            inotify_dup: 2,
            spawns: BTreeMap::from([("git".into(), 4), ("sh".into(), 1)]),
            cg_mem_peak: Some(8 * 1024 * 1024),
            cg_anon: Some(3 * 1024 * 1024),
            psi_cpu: Some(Pressure {
                some_avg10: 1.5,
                some_total_us: 40,
                full_avg10: 0.2,
                full_total_us: 4,
            }),
            proc_busy_us: 5_000_000 + 4_000_000,
            real_engine_cpu_us: Some(100_000 + 500_000),
            ..Rollup::default()
        };
        let set = SampleSet {
            root_pid: 1,
            hz: 4.0,
            containment: "scope",
            clk_tck: 100,
            ticks: vec![tick(0, start), tick(1_000_000_000, end)],
            marks: vec![],
            data_dir_growth: DataDirGrowth {
                sqlite: 10,
                ..DataDirGrowth::default()
            },
            extra_spawns: BTreeMap::from([("git".into(), 2)]),
            rpc_bytes_in: BTreeMap::from([("WatchDocMessages".into(), 99)]),
            rpc_items: BTreeMap::from([("WatchDocMessages".into(), 3)]),
        };
        let got = metrics(
            &set,
            Window {
                start_ns: 0,
                end_ns: 1_000_000_000,
            },
        );
        // Cgroup delta is 2s, not the proc delta.
        assert!((got.cpu_s.tree - 2.0).abs() < 1e-9, "{}", got.cpu_s.tree);
        assert!((got.cpu_s.user - 1.6).abs() < 1e-9);
        assert!((got.cpu_s.system - 0.4).abs() < 1e-9);
        assert!((got.cpu_s.engine - 0.03).abs() < 1e-9);
        assert!((got.cpu_s.by_group["git"] - 0.004).abs() < 1e-9);
        assert!((got.cpu_pct_core.tree - 200.0).abs() < 1e-6);
        assert!((got.pss_mib.peak - 3.0).abs() < 1e-9);
        assert!((got.pss_mib.end - 3.0).abs() < 1e-9);
        assert!((got.pss_mib.mean - 2.0).abs() < 1e-9);
        assert!((got.pss_mib.by_group_peak["engine"] - 2.0).abs() < 1e-9);
        assert!((got.pss_anon_mib.peak - 2.0).abs() < 1e-9);
        assert!((got.pss_file_mib.end - 0.5).abs() < 1e-9);
        assert!((got.swap_pss_mib.peak - 0.25).abs() < 1e-9);
        assert!((got.cg_mem_peak_mib.unwrap() - 8.0).abs() < 1e-9);
        assert!((got.cg_anon_mib.as_ref().unwrap().peak - 3.0).abs() < 1e-6);
        assert!((got.wakeups_per_s.tree - 200.0).abs() < 1e-9);
        assert!((got.wakeups_per_s.by_comm["tokio-runtime-w"] - 200.0).abs() < 1e-9);
        assert!((got.ctx_per_s.vol - 200.0).abs() < 1e-9);
        assert!((got.ctx_per_s.invol - 10.0).abs() < 1e-9);
        assert_eq!(got.write_bytes, 100);
        assert_eq!(got.wchar, 50);
        assert_eq!(got.syscw, 7);
        assert_eq!(got.threads.max, 9);
        assert_eq!(got.threads.end, 9);
        assert_eq!(got.threads.by_comm["tokio-runtime-w"], 8);
        assert_eq!(got.arena_heaps, 3);
        assert_eq!(got.fds, 12);
        assert_eq!(got.inotify.watches, 5);
        assert_eq!(got.inotify.dup, 2);
        assert_eq!(got.spawns["git"], 5);
        assert_eq!(got.spawns["sh"], 1);
        assert_eq!(got.data_dir_growth.sqlite, 10);
        assert_eq!(got.rpc.bytes_in["WatchDocMessages"], 99);
        assert_eq!(got.rpc.items["WatchDocMessages"], 3);
        // busy delta 4s, tree 2s, wall 1s → 200% of a core outside.
        assert!(
            (got.outside_cpu_pct - 200.0).abs() < 1e-6,
            "{}",
            got.outside_cpu_pct
        );
        assert!((got.real_engine_cpu_s.unwrap() - 0.5).abs() < 1e-9);
        let psi = got.psi_cpu.unwrap();
        assert_eq!(psi.some_total_us, 30);
        assert_eq!(psi.full_total_us, 3);
        assert!((psi.some_avg10 - 1.5).abs() < 1e-9);
        let _ = rollup_with(0, 0);
    }

    #[test]
    fn data_dir_snapshot_classifies_and_ignores_symlinks() {
        let root = scratch("data-dir");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("journals/nested")).unwrap();
        fs::create_dir_all(root.join("logs")).unwrap();
        fs::write(root.join("docs.sqlite3"), vec![1u8; 100]).unwrap();
        fs::write(root.join("docs.sqlite3-wal"), vec![2u8; 40]).unwrap();
        fs::write(root.join("journals/nested/j.journal"), vec![3u8; 7]).unwrap();
        fs::write(root.join("logs/engine.log"), vec![4u8; 5]).unwrap();
        fs::write(root.join("previews.json"), vec![5u8; 9]).unwrap();
        fs::write(root.join("notes.txt"), vec![6u8; 3]).unwrap();
        std::os::unix::fs::symlink(root.join("docs.sqlite3"), root.join("link.sqlite3")).unwrap();
        let sizes = snapshot_data_dir(&root).unwrap();
        assert_eq!(sizes.sqlite, 100);
        assert_eq!(sizes.wal, 40);
        assert_eq!(sizes.journals, 7);
        assert_eq!(sizes.logs, 5);
        assert_eq!(sizes.previews_json, 9);
        assert_eq!(sizes.other, 3);
        let mut later = sizes.clone();
        later.sqlite = 180;
        later.journals = 4;
        let growth = later.growth_since(&sizes);
        assert_eq!(growth.sqlite, 80);
        assert_eq!(growth.journals, 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn live_sleep_is_idle_and_visible() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let guard = KillChild {
            child: &mut child,
            group: false,
        };
        let handle = Sampler::start(guard.child.id(), None, 10.0, &Grouping::default()).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        handle.mark(Mark::WindowStart).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        handle.mark(Mark::WindowEnd).unwrap();
        let set = handle.finish().unwrap();
        let window = set.marked_window().unwrap();
        let got = metrics(&set, window);
        assert!(got.cpu_s.tree < 0.05, "sleep cpu {}", got.cpu_s.tree);
        assert!(got.pss_mib.end > 0.0, "sleep should have a PSS");
        let seen = set
            .ticks
            .iter()
            .any(|tick| tick.procs.iter().any(|proc| proc.pid == guard.child.id()));
        assert!(seen, "sleep pid missing from samples");
        assert_eq!(set.containment, "pgroup");
    }

    #[test]
    fn live_busy_loop_cpu_is_within_3_percent() {
        let bin = helper("busy", BUSY_C);
        let mut child = std::process::Command::new(&bin)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        wait_ready(&stdout);
        let guard = KillChild {
            child: &mut child,
            group: false,
        };
        let handle = Sampler::start(guard.child.id(), None, 20.0, &Grouping::default()).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        handle.mark(Mark::WindowStart).unwrap();
        let ticks0 = proc_cpu_ticks(guard.child.id());
        std::thread::sleep(Duration::from_millis(2000));
        let ticks1 = proc_cpu_ticks(guard.child.id());
        handle.mark(Mark::WindowEnd).unwrap();
        let set = handle.finish().unwrap();
        let got = metrics(&set, set.marked_window().unwrap());
        let truth = (ticks1 - ticks0) as f64 / clk_tck() as f64;
        let rel = (got.cpu_s.tree - truth).abs() / truth;
        assert!(truth > 1.5, "busy child only accumulated {truth}s");
        assert!(
            rel < 0.03,
            "sampler {}s truth {truth}s rel {rel}",
            got.cpu_s.tree
        );
        assert!(
            (got.cpu_s.engine - got.cpu_s.tree).abs() < 0.02,
            "engine {} tree {}",
            got.cpu_s.engine,
            got.cpu_s.tree
        );
        let _ = stdout;
    }

    #[test]
    fn live_touch_100_mib_pss_is_within_5_percent() {
        let bin = helper("touch100", TOUCH_C);
        let mut child = std::process::Command::new(&bin)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        wait_ready(&stdout);
        let guard = KillChild {
            child: &mut child,
            group: false,
        };
        let handle = Sampler::start(guard.child.id(), None, 10.0, &Grouping::default()).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        handle.mark(Mark::WindowStart).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        handle.mark(Mark::WindowEnd).unwrap();
        let set = handle.finish().unwrap();
        let got = metrics(&set, set.marked_window().unwrap());
        let low = 100.0 * 0.95;
        let high = 100.0 * 1.05;
        assert!(
            got.pss_mib.end >= low && got.pss_mib.end <= high,
            "PSS end {} MiB, anon {} peak {}",
            got.pss_mib.end,
            got.pss_anon_mib.end,
            got.pss_mib.peak
        );
        assert!(
            got.pss_mib.peak >= low && got.pss_mib.peak <= high,
            "PSS peak {}",
            got.pss_mib.peak
        );
    }

    #[test]
    fn live_50_process_tree_at_4hz_uses_under_one_percent_of_a_core() {
        let bin = helper("tree50", TREE_C);
        let mut child = std::process::Command::new(&bin)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        wait_ready(&stdout);
        let guard = KillChild {
            child: &mut child,
            group: true,
        };
        let handle = Sampler::start(guard.child.id(), None, 4.0, &Grouping::default()).unwrap();
        let tid = wait_tid(&handle);
        std::thread::sleep(Duration::from_millis(500));
        let before = thread_run_ns(tid);
        let started = std::time::Instant::now();
        std::thread::sleep(Duration::from_secs(8));
        let after = thread_run_ns(tid);
        let wall = started.elapsed().as_secs_f64();
        handle.mark(Mark::WindowEnd).unwrap();
        let set = handle.finish().unwrap();
        let cpu_s = (after.saturating_sub(before)) as f64 / 1e9;
        let pct = cpu_s / wall * 100.0;
        let last = set.ticks.last().unwrap();
        assert_eq!(
            last.tree_pids, 50,
            "expected 50 live pids, saw {}",
            last.tree_pids
        );
        let samples_in_window = set
            .ticks
            .iter()
            .filter(|tick| tick.t_ns >= before_window_hint(&set))
            .count();
        assert!(
            set.ticks.len() >= 24,
            "sampler produced only {} ticks at 4 Hz",
            set.ticks.len()
        );
        assert!(
            pct < 1.0,
            "sampler used {pct:.3}% of a core ({cpu_s:.4}s over {wall:.2}s, {} ticks)",
            set.ticks.len()
        );
        let _ = samples_in_window;
    }

    #[test]
    fn live_fake_cgroup_cpu_and_memory_peak_are_used() {
        let dir = scratch("fake-cg");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("cpu.stat"),
            "usage_usec 1000000\nuser_usec 800000\nsystem_usec 200000\n",
        )
        .unwrap();
        fs::write(dir.join("memory.current"), "1048576\n").unwrap();
        fs::write(dir.join("memory.peak"), "1048576\n").unwrap();
        fs::write(dir.join("memory.stat"), "anon 524288\nfile 1\n").unwrap();
        fs::write(
            dir.join("cpu.pressure"),
            "some avg10=0.00 avg60=0.00 avg300=0.00 total=10\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=1\n",
        )
        .unwrap();
        fs::write(
            dir.join("memory.pressure"),
            "some avg10=0.00 avg60=0.00 avg300=0.00 total=4\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n",
        )
        .unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let guard = KillChild {
            child: &mut child,
            group: false,
        };
        fs::write(dir.join("cgroup.procs"), format!("{}\n", guard.child.id())).unwrap();
        let scope = Cgroup::open(&dir).unwrap();
        let handle =
            Sampler::start(guard.child.id(), Some(scope), 10.0, &Grouping::default()).unwrap();
        handle.mark(Mark::WindowStart).unwrap();
        fs::write(
            dir.join("cpu.stat"),
            "usage_usec 3000000\nuser_usec 2400000\nsystem_usec 600000\n",
        )
        .unwrap();
        fs::write(dir.join("memory.peak"), format!("{}\n", 10 * 1024 * 1024)).unwrap();
        fs::write(
            dir.join("memory.stat"),
            format!("anon {}\nfile 1\n", 6 * 1024 * 1024),
        )
        .unwrap();
        fs::write(
            dir.join("cpu.pressure"),
            "some avg10=1.25 avg60=0.00 avg300=0.00 total=50\nfull avg10=0.10 avg60=0.00 avg300=0.00 total=6\n",
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        handle.mark(Mark::WindowEnd).unwrap();
        let set = handle.finish().unwrap();
        assert_eq!(set.containment, "scope");
        let got = metrics(&set, set.marked_window().unwrap());
        assert!(
            (got.cpu_s.tree - 2.0).abs() < 1e-6,
            "tree {}",
            got.cpu_s.tree
        );
        assert!(
            (got.cg_mem_peak_mib.unwrap() - 10.0).abs() < 1e-6,
            "{got:?}"
        );
        assert!(got.cg_anon_mib.as_ref().unwrap().peak > 5.0);
        let psi = got.psi_cpu.unwrap();
        assert!(psi.some_total_us >= 40, "{}", psi.some_total_us);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_self_threads_sum_wakeups_by_comm() {
        let pid = std::process::id();
        let handle = Sampler::start(pid, None, 10.0, &Grouping::default()).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        handle.mark(Mark::WindowStart).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        handle.mark(Mark::WindowEnd).unwrap();
        let set = handle.finish().unwrap();
        let got = metrics(&set, set.marked_window().unwrap());
        assert!(
            got.threads.max >= 2,
            "expected the test process to be multi-threaded, max {}",
            got.threads.max
        );
        assert!(
            got.threads.by_comm.contains_key("zb-sample"),
            "histogram {:?}",
            got.threads.by_comm
        );
        let named: u32 = got.threads.by_comm.values().copied().sum();
        assert!(named >= 2, "histogram {:?}", got.threads.by_comm);
        assert!(
            got.wakeups_per_s.tree > 0.0,
            "timeslices did not advance ({})",
            got.wakeups_per_s.tree
        );
    }

    struct KillChild<'a> {
        child: &'a mut std::process::Child,
        group: bool,
    }

    impl Drop for KillChild<'_> {
        fn drop(&mut self) {
            if self.group {
                unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn proc_cpu_ticks(pid: u32) -> u64 {
        let text = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let stat = parse_stat(&text).unwrap();
        stat.utime + stat.stime
    }

    fn thread_run_ns(tid: u32) -> u64 {
        let text = fs::read_to_string(format!("/proc/self/task/{tid}/schedstat")).unwrap();
        proc::parse_schedstat(&text).unwrap().run_ns
    }

    fn wait_tid(handle: &Handle) -> u32 {
        for _ in 0..50 {
            if let Some(tid) = handle.sampler_tid() {
                return tid;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("sampler tid was not published");
    }

    fn wait_ready(stdout: &impl AsRawFd) {
        let fd = stdout.as_raw_fd();
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut pfd, 1, 5000) };
        assert!(rc > 0, "helper did not become ready");
        let mut buf = [0u8; 32];
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        assert!(n > 0, "helper stdout closed");
        let n = n as usize;
        assert!(
            buf[..n].windows(5).any(|window| window == b"ready"),
            "unexpected helper output {}",
            String::from_utf8_lossy(&buf[..n])
        );
    }

    fn before_window_hint(set: &SampleSet) -> u64 {
        set.ticks.first().map(|tick| tick.t_ns).unwrap_or(0)
    }

    fn scratch(name: &str) -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".into());
        let dir = PathBuf::from(home)
            .join("Documents/dev/tmp/zeron-perf/scratch/b2-sampler")
            .join(format!("{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn helper(name: &str, source: &str) -> PathBuf {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let dir = {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".into());
            let dir =
                PathBuf::from(home).join("Documents/dev/tmp/zeron-perf/scratch/b2-sampler/helpers");
            fs::create_dir_all(&dir).unwrap();
            dir
        };
        let src = dir.join(format!("{name}.c"));
        let bin = dir.join(name);
        let stale = fs::read_to_string(&src).ok().as_deref() != Some(source) || !bin.is_file();
        if stale {
            fs::write(&src, source).unwrap();
            let status = std::process::Command::new("cc")
                .args([
                    "-std=c11",
                    "-Os",
                    "-static",
                    "-nostdlib",
                    "-fno-stack-protector",
                    "-fno-pie",
                    "-no-pie",
                    "-o",
                ])
                .arg(&bin)
                .arg(&src)
                .status()
                .expect("cc");
            assert!(status.success(), "compile {name} failed");
        }
        bin
    }

    const BUSY_C: &str = r#"
static long sc(long n, long a, long b, long c, long d, long e, long f) {
    register long r10 __asm__("r10") = d;
    register long r8 __asm__("r8") = e;
    register long r9 __asm__("r9") = f;
    long ret;
    __asm__ volatile("syscall" : "=a"(ret) : "a"(n), "D"(a), "S"(b), "d"(c), "r"(r10), "r"(r8), "r"(r9) : "rcx", "r11", "memory");
    return ret;
}
void _start(void) {
    const char msg[] = "ready\n";
    sc(1, 1, (long)msg, 6, 0, 0, 0);
    volatile unsigned long x = 0;
    for (;;) x++;
}
"#;

    const TOUCH_C: &str = r#"
static long sc(long n, long a, long b, long c, long d, long e, long f) {
    register long r10 __asm__("r10") = d;
    register long r8 __asm__("r8") = e;
    register long r9 __asm__("r9") = f;
    long ret;
    __asm__ volatile("syscall" : "=a"(ret) : "a"(n), "D"(a), "S"(b), "d"(c), "r"(r10), "r"(r8), "r"(r9) : "rcx", "r11", "memory");
    return ret;
}
void _start(void) {
    long n = 100L * 1024L * 1024L;
    long p = sc(9, 0, n, 3, 0x22, -1, 0);
    if ((unsigned long)p > 0xfffffffffffff000UL) sc(60, 1, 0, 0, 0, 0, 0);
    volatile char *b = (volatile char *)p;
    for (long i = 0; i < n; i += 4096) b[i] = 1;
    const char msg[] = "ready\n";
    sc(1, 1, (long)msg, 6, 0, 0, 0);
    for (;;) sc(34, 0, 0, 0, 0, 0, 0);
}
"#;

    const TREE_C: &str = r#"
static long sc(long n, long a, long b, long c, long d, long e, long f) {
    register long r10 __asm__("r10") = d;
    register long r8 __asm__("r8") = e;
    register long r9 __asm__("r9") = f;
    long ret;
    __asm__ volatile("syscall" : "=a"(ret) : "a"(n), "D"(a), "S"(b), "d"(c), "r"(r10), "r"(r8), "r"(r9) : "rcx", "r11", "memory");
    return ret;
}
void _start(void) {
    sc(109, 0, 0, 0, 0, 0, 0);
    /* Drop fds inherited from the test harness. The budget is for a
       50-process tree, and readlink of a harness fd table dominates. */
    for (int fd = 3; fd < 1024; fd++) sc(3, fd, 0, 0, 0, 0, 0);
    for (int i = 0; i < 49; i++) {
        long pid = sc(57, 0, 0, 0, 0, 0, 0);
        if (pid < 0) sc(60, 1, 0, 0, 0, 0, 0);
        if (pid == 0) {
            sc(157, 1, 9, 0, 0, 0, 0);
            if (sc(110, 0, 0, 0, 0, 0, 0) == 1) sc(60, 0, 0, 0, 0, 0, 0);
            for (;;) sc(34, 0, 0, 0, 0, 0, 0);
        }
    }
    const char msg[] = "ready\n";
    sc(1, 1, (long)msg, 6, 0, 0, 0);
    for (;;) sc(34, 0, 0, 0, 0, 0, 0);
}
"#;
}
