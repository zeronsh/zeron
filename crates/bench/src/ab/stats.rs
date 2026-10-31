//! Paired-difference statistics, the self-pair noise floor, and the verdict.
//!
//! A pair is the two neighbouring trials of one ABBA round. The paired
//! change is `(B − A) / A × 100`. `noise_pct` is the nearest-rank 90th
//! percentile of `|paired Δ|` from a fresh self-pair control.
//!
//! `better` or `worse` requires both: the A and B ranges do not overlap, or
//! every paired difference has the same strict sign; and `|median paired Δ|`
//! is greater than `noise_pct`. Anything else is `within noise`. A failed
//! correctness check forces `invalid`. A missing or stale control leaves
//! `noise_pct` empty, so the verdict stays `within noise`.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::schedule::{self, Side, median};
use super::{Check, MetricView, TrialRecord};

/// Thirty days. Older than this, the control is not fresh.
pub const NOISE_MAX_AGE_SECS: u64 = 30 * 24 * 60 * 60;

/// Words the report is allowed to print for a metric.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    #[serde(rename = "better")]
    Better,
    #[serde(rename = "worse")]
    Worse,
    #[serde(rename = "within noise")]
    WithinNoise,
    #[serde(rename = "invalid")]
    Invalid,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Better => "better",
            Self::Worse => "worse",
            Self::WithinNoise => "within noise",
            Self::Invalid => "invalid",
        }
    }
}

/// Machine identity stored next to a noise file. A mismatch means unknown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub machine: String,
    pub kernel: String,
    pub governor: String,
    pub epp: String,
    pub bios: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SideStats {
    pub median: f64,
    pub min: f64,
    pub max: f64,
    pub n: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeltaPct {
    pub median: f64,
    pub min: f64,
    pub max: f64,
}

/// One row of `summary.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetricSummary {
    #[serde(rename = "A")]
    pub a: SideStats,
    #[serde(rename = "B")]
    pub b: SideStats,
    pub paired_delta_pct: DeltaPct,
    /// `None` when the control is missing, stale, or has no entry for this id.
    pub noise_pct: Option<f64>,
    pub verdict: Verdict,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScenarioStats {
    pub main: BTreeMap<String, MetricSummary>,
    /// Full ids `wakeups_per_s.comm.<name>`.
    pub wakeups: BTreeMap<String, MetricSummary>,
    /// Full ids `pss_mib.group.<name>`.
    pub pss_groups: BTreeMap<String, MetricSummary>,
    pub counters: BTreeMap<String, MetricSummary>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NoiseStatus {
    Fresh(BTreeMap<String, f64>),
    /// The report says "unknown". The tag is why, for the header note.
    Unknown(&'static str),
}

impl NoiseStatus {
    pub fn get(&self, id: &str) -> Option<f64> {
        match self {
            Self::Fresh(map) => map.get(id).copied(),
            Self::Unknown(_) => None,
        }
    }

    pub fn note(&self) -> String {
        match self {
            Self::Fresh(_) => "fresh".into(),
            Self::Unknown(why) => format!("unknown ({why})"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct NoiseFile {
    scenario: String,
    variant: String,
    recorded_unix: u64,
    machine: String,
    kernel: String,
    governor: String,
    epp: String,
    bios: String,
    /// `noise_pct` per metric id, including counters.
    metrics: BTreeMap<String, f64>,
}

/// `(B − A) / A × 100`. Both zero is 0%. A zero baseline is not a percentage.
pub fn delta_pct(a: f64, b: f64) -> Option<f64> {
    if !a.is_finite() || !b.is_finite() {
        return None;
    }
    if a == 0.0 {
        return if b == 0.0 { Some(0.0) } else { None };
    }
    Some((b - a) / a * 100.0)
}

/// NIST nearest-rank percentile. `p` is in `0..=1`.
/// The rank is `ceil(p × n)`, 1-based, into the ascending sample.
pub fn percentile_nearest_rank(values: &[f64], p: f64) -> Option<f64> {
    if !(0.0..=1.0).contains(&p) {
        return None;
    }
    let mut finite: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if finite.is_empty() {
        return None;
    }
    finite.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = finite.len();
    let rank = ((p * n as f64).ceil() as usize).clamp(1, n);
    Some(finite[rank - 1])
}

/// 90th percentile of the absolute paired changes, in percent.
pub fn noise_pct_of(abs_delta_pcts: &[f64]) -> Option<f64> {
    percentile_nearest_rank(abs_delta_pcts, 0.90)
}

pub fn checks_failed(trials: &[TrialRecord]) -> bool {
    trials
        .iter()
        .any(|trial| !trial.valid || trial.checks.iter().any(|check| !check.ok))
}

pub fn failed_checks(trials: &[TrialRecord]) -> Vec<(String, &Check)> {
    let mut out = Vec::new();
    for trial in trials {
        for check in &trial.checks {
            if !check.ok {
                out.push((trial.dir_name(), check));
            }
        }
    }
    out
}

/// Summarise measured pairs. Warm-ups (`pair_index == None`) are ignored.
///
/// `noise` is `None` for a whole-run unknown control. A `Some` map still
/// leaves a metric unknown when its id is absent.
pub fn summarize_trials(
    trials: &[TrialRecord],
    noise: Option<&BTreeMap<String, f64>>,
) -> ScenarioStats {
    let failed = checks_failed(trials);
    let measured: Vec<&TrialRecord> = trials.iter().filter(|t| t.pair_index.is_some()).collect();
    let mut metric_ids = BTreeMap::<String, ()>::new();
    let mut counter_ids = BTreeMap::<String, ()>::new();
    for trial in &measured {
        for id in trial.metrics.ids() {
            metric_ids.insert(id, ());
        }
        for id in trial.counters_delta.keys() {
            counter_ids.insert(id.clone(), ());
        }
    }
    let mut stats = ScenarioStats {
        main: BTreeMap::new(),
        wakeups: BTreeMap::new(),
        pss_groups: BTreeMap::new(),
        counters: BTreeMap::new(),
    };
    for id in metric_ids.keys() {
        let Some(summary) = summarize_id(&measured, id, false, noise, failed) else {
            continue;
        };
        if id.starts_with("wakeups_per_s.comm.") {
            stats.wakeups.insert(id.clone(), summary);
        } else if id.starts_with("pss_mib.group.") {
            stats.pss_groups.insert(id.clone(), summary);
        } else {
            stats.main.insert(id.clone(), summary);
        }
    }
    for id in counter_ids.keys() {
        if let Some(summary) = summarize_id(&measured, id, true, noise, failed) {
            stats.counters.insert(id.clone(), summary);
        }
    }
    stats
}

fn summarize_id(
    measured: &[&TrialRecord],
    id: &str,
    counter: bool,
    noise: Option<&BTreeMap<String, f64>>,
    checks_failed: bool,
) -> Option<MetricSummary> {
    let mut a_by_pair: BTreeMap<u32, f64> = BTreeMap::new();
    let mut b_by_pair: BTreeMap<u32, f64> = BTreeMap::new();
    for trial in measured {
        let Some(pair) = trial.pair_index else {
            continue;
        };
        let value = if counter {
            trial.counters_delta.get(id).copied()
        } else {
            trial.metrics.get(id)
        };
        let Some(value) = value else {
            continue;
        };
        if !value.is_finite() {
            continue;
        }
        match trial.side {
            Side::A => {
                a_by_pair.insert(pair, value);
            }
            Side::B => {
                b_by_pair.insert(pair, value);
            }
        }
    }
    let mut a_vals = Vec::new();
    let mut b_vals = Vec::new();
    let mut raw = Vec::new();
    let mut pcts = Vec::new();
    for (pair, a) in &a_by_pair {
        let Some(b) = b_by_pair.get(pair) else {
            continue;
        };
        a_vals.push(*a);
        b_vals.push(*b);
        raw.push(*b - *a);
        if let Some(pct) = delta_pct(*a, *b) {
            pcts.push(pct);
        }
    }
    if a_vals.is_empty() {
        return None;
    }
    // A pair with a zero baseline and a non-zero candidate has no percentage.
    // That metric cannot clear the noise floor.
    let comparable = pcts.len() == raw.len();
    let pcts = if comparable { pcts } else { Vec::new() };
    let noise_pct = noise.and_then(|map| map.get(id).copied());
    let summary = MetricSummary {
        a: side_stats(&a_vals)?,
        b: side_stats(&b_vals)?,
        paired_delta_pct: delta_stats(&pcts).unwrap_or(DeltaPct {
            median: 0.0,
            min: 0.0,
            max: 0.0,
        }),
        noise_pct,
        verdict: decide(checks_failed, &a_vals, &b_vals, &raw, &pcts, noise_pct),
    };
    Some(summary)
}

fn side_stats(values: &[f64]) -> Option<SideStats> {
    if values.is_empty() {
        return None;
    }
    Some(SideStats {
        median: median(values)?,
        min: values.iter().copied().fold(f64::INFINITY, f64::min),
        max: values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        n: values.len() as u32,
    })
}

fn delta_stats(pcts: &[f64]) -> Option<DeltaPct> {
    if pcts.is_empty() {
        return None;
    }
    Some(DeltaPct {
        median: median(pcts)?,
        min: pcts.iter().copied().fold(f64::INFINITY, f64::min),
        max: pcts.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    })
}

/// Lower is an improvement for every §2.1 cost.
fn decide(
    checks_failed: bool,
    a: &[f64],
    b: &[f64],
    raw: &[f64],
    pcts: &[f64],
    noise_pct: Option<f64>,
) -> Verdict {
    if checks_failed {
        return Verdict::Invalid;
    }
    let Some(noise_pct) = noise_pct else {
        return Verdict::WithinNoise;
    };
    let Some(med) = median(pcts) else {
        return Verdict::WithinNoise;
    };
    let same_sign =
        !raw.is_empty() && (raw.iter().all(|d| *d > 0.0) || raw.iter().all(|d| *d < 0.0));
    let a_min = a.iter().copied().fold(f64::INFINITY, f64::min);
    let a_max = a.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let b_min = b.iter().copied().fold(f64::INFINITY, f64::min);
    let b_max = b.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let no_overlap = a_max < b_min || b_max < a_min;
    if (same_sign || no_overlap) && med.abs() > noise_pct {
        if med < 0.0 {
            Verdict::Better
        } else {
            Verdict::Worse
        }
    } else {
        Verdict::WithinNoise
    }
}

/// `|paired Δ%|` per metric id, for a self-pair control.
pub fn noise_from_trials(trials: &[TrialRecord]) -> BTreeMap<String, f64> {
    let measured: Vec<&TrialRecord> = trials.iter().filter(|t| t.pair_index.is_some()).collect();
    let mut ids = BTreeMap::<String, bool>::new();
    for trial in &measured {
        for id in trial.metrics.ids() {
            ids.insert(id, false);
        }
        for id in trial.counters_delta.keys() {
            ids.insert(id.clone(), true);
        }
    }
    let mut out = BTreeMap::new();
    for (id, counter) in ids {
        let mut abs = Vec::new();
        let mut a_by = BTreeMap::<u32, f64>::new();
        let mut b_by = BTreeMap::<u32, f64>::new();
        for trial in &measured {
            let Some(pair) = trial.pair_index else {
                continue;
            };
            let value = if counter {
                trial.counters_delta.get(&id).copied()
            } else {
                trial.metrics.get(&id)
            };
            let Some(value) = value else {
                continue;
            };
            match trial.side {
                Side::A => {
                    a_by.insert(pair, value);
                }
                Side::B => {
                    b_by.insert(pair, value);
                }
            }
        }
        for (pair, a) in &a_by {
            let Some(b) = b_by.get(pair) else {
                continue;
            };
            if let Some(pct) = delta_pct(*a, *b) {
                abs.push(pct.abs());
            }
        }
        if let Some(noise) = noise_pct_of(&abs) {
            out.insert(id, noise);
        }
    }
    out
}

pub fn noise_path(noise_dir: &Path, scenario: &str, variant_key: &str) -> std::path::PathBuf {
    let name = format!(
        "{}-{}.json",
        crate::env::sanitize(scenario),
        crate::env::sanitize(variant_key)
    );
    noise_dir.join(name)
}

pub fn store_noise(
    noise_dir: &Path,
    scenario: &str,
    variant_key: &str,
    fingerprint: &Fingerprint,
    now_unix: u64,
    metrics: &BTreeMap<String, f64>,
) -> Result<std::path::PathBuf> {
    fs::create_dir_all(noise_dir).with_context(|| format!("create {}", noise_dir.display()))?;
    let path = noise_path(noise_dir, scenario, variant_key);
    let file = NoiseFile {
        scenario: scenario.to_string(),
        variant: variant_key.to_string(),
        recorded_unix: now_unix,
        machine: fingerprint.machine.clone(),
        kernel: fingerprint.kernel.clone(),
        governor: fingerprint.governor.clone(),
        epp: fingerprint.epp.clone(),
        bios: fingerprint.bios.clone(),
        metrics: metrics.clone(),
    };
    let body = serde_json::to_vec_pretty(&file).context("encode noise file")?;
    fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

pub fn lookup_noise(
    noise_dir: &Path,
    scenario: &str,
    variant_key: &str,
    fingerprint: &Fingerprint,
    now_unix: u64,
) -> NoiseStatus {
    let path = noise_path(noise_dir, scenario, variant_key);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return NoiseStatus::Unknown("missing"),
    };
    let file: NoiseFile = match serde_json::from_str(&text) {
        Ok(file) => file,
        Err(_) => return NoiseStatus::Unknown("unreadable"),
    };
    if now_unix.saturating_sub(file.recorded_unix) > NOISE_MAX_AGE_SECS {
        return NoiseStatus::Unknown("stale");
    }
    if file.machine != fingerprint.machine {
        return NoiseStatus::Unknown("machine");
    }
    if file.kernel != fingerprint.kernel {
        return NoiseStatus::Unknown("kernel");
    }
    if file.governor != fingerprint.governor {
        return NoiseStatus::Unknown("governor");
    }
    if file.epp != fingerprint.epp {
        return NoiseStatus::Unknown("epp");
    }
    if file.bios != fingerprint.bios {
        return NoiseStatus::Unknown("bios");
    }
    NoiseStatus::Fresh(file.metrics)
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Spread check over headline ids. Warm-ups are already absent from `values`
/// when the caller filters them.
pub fn headline_needs_pair(samples: &[(Side, &str, Vec<f64>)]) -> bool {
    let _ = schedule::SPREAD_LIMIT;
    samples
        .iter()
        .any(|(_, _, values)| schedule::exceeds_spread(values))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(a: f64, b: f64) -> (f64, f64) {
        (a, b)
    }

    fn verdict_of(pairs: &[(f64, f64)], noise: Option<f64>, failed: bool) -> Verdict {
        let a: Vec<f64> = pairs.iter().map(|p| p.0).collect();
        let b: Vec<f64> = pairs.iter().map(|p| p.1).collect();
        let raw: Vec<f64> = pairs.iter().map(|p| p.1 - p.0).collect();
        let pcts: Vec<f64> = pairs.iter().filter_map(|p| delta_pct(p.0, p.1)).collect();
        decide(failed, &a, &b, &raw, &pcts, noise)
    }

    #[test]
    fn nearest_rank_90th_and_verdict_rule() {
        assert_eq!(
            percentile_nearest_rank(&[1.0, 2.0, 3.0, 4.0, 10.0], 0.90),
            Some(10.0)
        );
        assert_eq!(percentile_nearest_rank(&[1.0, 10.0], 0.90), Some(10.0));
        // Clear drop, no overlap, above the floor.
        assert_eq!(
            verdict_of(&[pair(100.0, 50.0), pair(100.0, 50.0)], Some(5.0), false),
            Verdict::Better
        );
        // Clear rise.
        assert_eq!(
            verdict_of(&[pair(50.0, 100.0), pair(50.0, 100.0)], Some(5.0), false),
            Verdict::Worse
        );
        // Same sign and no overlap, but the median change is inside the floor.
        assert_eq!(
            verdict_of(&[pair(100.0, 99.0), pair(100.0, 99.0)], Some(5.0), false),
            Verdict::WithinNoise
        );
        // Ranges overlap and the change is small.
        assert_eq!(
            verdict_of(&[pair(100.0, 101.0), pair(102.0, 103.0)], Some(5.0), false),
            Verdict::WithinNoise
        );
        // No control: do not claim a winner.
        assert_eq!(
            verdict_of(&[pair(100.0, 50.0), pair(100.0, 50.0)], None, false),
            Verdict::WithinNoise
        );
        // A failed check wins over the numbers.
        assert_eq!(
            verdict_of(&[pair(100.0, 50.0)], Some(1.0), true),
            Verdict::Invalid
        );
        let json = serde_json::to_value(Verdict::WithinNoise).unwrap();
        assert_eq!(json, serde_json::json!("within noise"));
    }

    #[test]
    fn noise_lookup_age_and_identity() {
        let dir = scratch("noise");
        let fp = Fingerprint {
            machine: "rig".into(),
            kernel: "6.8".into(),
            governor: "powersave".into(),
            epp: "balance_power".into(),
            bios: "1.0".into(),
        };
        let now = 1_700_000_000u64;
        let mut metrics = BTreeMap::new();
        metrics.insert("cpu_s.engine".into(), 4.0);
        store_noise(&dir, "fake", "injected", &fp, now, &metrics).unwrap();
        match lookup_noise(&dir, "fake", "injected", &fp, now) {
            NoiseStatus::Fresh(map) => assert_eq!(map["cpu_s.engine"], 4.0),
            NoiseStatus::Unknown(why) => panic!("expected fresh, got {why}"),
        }
        let later = now + NOISE_MAX_AGE_SECS + 1;
        assert_eq!(
            lookup_noise(&dir, "fake", "injected", &fp, later),
            NoiseStatus::Unknown("stale")
        );
        let mut other = fp.clone();
        other.kernel = "9.9".into();
        assert_eq!(
            lookup_noise(&dir, "fake", "injected", &other, now),
            NoiseStatus::Unknown("kernel")
        );
        other = fp.clone();
        other.governor = "performance".into();
        assert_eq!(
            lookup_noise(&dir, "fake", "injected", &other, now),
            NoiseStatus::Unknown("governor")
        );
        other = fp.clone();
        other.bios = "2.0".into();
        assert_eq!(
            lookup_noise(&dir, "fake", "injected", &other, now),
            NoiseStatus::Unknown("bios")
        );
        assert_eq!(
            lookup_noise(&dir, "fake", "missing", &fp, now),
            NoiseStatus::Unknown("missing")
        );
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from("/home/user")
            .join("Documents/dev/tmp/zeron-perf/scratch/b6-ab-orchestrator/tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
