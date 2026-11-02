//! `summary.json` and `report.md`.
//!
//! The markdown table is `metric | A median [min–max] | B median [min–max] |
//! Δ paired median [range] | noise | verdict`. Headline metrics come first.
//! Counters sit in a collapsible section. S-idle adds a per-thread wakeup
//! table and S-mcp a per-group PSS table. `energy_j_net` is a row only when
//! a trial recorded it. There is no S-build section.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::stats::{MetricSummary, ScenarioStats};
use super::{MetricView, TrialRecord};

pub const TABLE_HEADER: &str = "| metric | A median [min–max] | B median [min–max] | Δ paired median [range] | noise | verdict |";
const TABLE_RULE: &str = "|---|---|---|---|---|---|";

/// Headline §2.1 costs, in report order. Optional ones appear only when set.
pub fn headline_metric_ids() -> &'static [&'static str] {
    &[
        "cpu_s.tree",
        "cpu_s.engine",
        "instructions_u",
        "pss_mib.peak",
        "wakeups_per_s.tree",
        "write_bytes",
        "latency_ms.stream.p50",
    ]
}

/// Scenario-specific headline metrics control both report ordering and the
/// automatic extra-pair rule. Other collected metrics remain secondary.
pub fn headline_metric_ids_for(scenario: &str) -> &'static [&'static str] {
    if scenario.eq_ignore_ascii_case("S-mcp") || scenario.eq_ignore_ascii_case("mcp") {
        &[
            "scenario.synthetic_mcp_pss_marginal_mib",
            "scenario.synthetic_session_tree_pss_marginal_mib",
            "scenario.synthetic_tool_cold_ms",
        ]
    } else {
        headline_metric_ids()
    }
}

pub fn render_report(
    meta: &super::RunMeta,
    stats: &ScenarioStats,
    trials: &[TrialRecord],
    noise_note: &str,
) -> String {
    let measured: Vec<&TrialRecord> = trials.iter().filter(|t| t.pair_index.is_some()).collect();
    let warmups = trials.len().saturating_sub(measured.len());
    let invalid = measured
        .iter()
        .filter(|t| !t.valid || t.checks.iter().any(|c| !c.ok))
        .count();
    let noisy = measured.iter().filter(|t| trial_noisy(t)).count();
    let bin = |side: &str| meta.binaries.get(side);
    let a = bin("A");
    let b = bin("B");
    let mut out = String::new();
    out.push_str(&format!("# zeron-bench {}\n\n", meta.label));
    out.push_str(&format!(
        "- revisions: A `{}` `{}`; B `{}` `{}`\n",
        a.map(|b| b.rev.as_str()).unwrap_or("-"),
        a.map(|b| b.sha256.as_str()).unwrap_or("-"),
        b.map(|b| b.rev.as_str()).unwrap_or("-"),
        b.map(|b| b.sha256.as_str()).unwrap_or("-"),
    ));
    out.push_str(&format!("- machine: {}\n", meta.machine));
    out.push_str(&format!("- kernel: {}\n", meta.kernel));
    out.push_str(&format!("- governor: {}\n", meta.governor));
    out.push_str(&format!("- epp: {}\n", meta.epp));
    out.push_str(&format!("- amd-pstate: {}\n", meta.amd_pstate));
    out.push_str(&format!("- fixtures: `{}`\n", meta.fixture_id));
    out.push_str(&format!("- tier: {}\n", meta.tier));
    out.push_str(&format!(
        "- trials: {measured} measured, {warmups} warm-up; invalid: {invalid}; noisy: {noisy}\n",
        measured = measured.len(),
    ));
    out.push_str(&format!("- noise: {noise_note}\n"));
    if let Some(note) = &meta.recovery_note {
        out.push_str(&format!("- recovery: {note}\n"));
    }
    out.push('\n');
    out.push_str(&format!("## {} / {}\n\n", meta.scenario, meta.variant_key));
    if meta.scenario.eq_ignore_ascii_case("S-mcp") {
        out.push_str("The empty fleet excludes synthetic servers but retains Zeron's built-in MCP server. Readiness is a polling upper bound, not a direct CLI mcp_status timestamp. Cold-call measurements use the synthetic server directly on A and the standalone proxy on B.\n\n");
    }
    push_table(
        &mut out,
        &main_ids(meta.scenario.as_str(), stats),
        stats_lookup(stats),
    );
    if meta.scenario.starts_with("S-idle") && !stats.wakeups.is_empty() {
        out.push_str("\n### Wakeups by thread\n\n");
        let ids: Vec<String> = stats.wakeups.keys().cloned().collect();
        push_table(&mut out, &ids, |id| stats.wakeups.get(id));
    }
    if meta.scenario.starts_with("S-mcp") && !stats.pss_groups.is_empty() {
        out.push_str("\n### PSS by group\n\n");
        let ids: Vec<String> = stats.pss_groups.keys().cloned().collect();
        push_table(&mut out, &ids, |id| stats.pss_groups.get(id));
    }
    if !stats.counters.is_empty() {
        out.push_str("\n<details>\n<summary>counters</summary>\n\n");
        let ids: Vec<String> = stats.counters.keys().cloned().collect();
        push_table(&mut out, &ids, |id| stats.counters.get(id));
        out.push_str("\n</details>\n");
    }
    out.push_str("\n## Check failures\n\n");
    let mut any = false;
    for trial in trials {
        for check in &trial.checks {
            if check.ok {
                continue;
            }
            any = true;
            match &check.detail {
                Some(detail) => {
                    out.push_str(&format!(
                        "- `{}` {}: {detail}\n",
                        trial.dir_name(),
                        check.name
                    ));
                }
                None => {
                    out.push_str(&format!("- `{}` {} failed\n", trial.dir_name(), check.name));
                }
            }
        }
        if !trial.valid && trial.checks.iter().all(|c| c.ok) {
            any = true;
            let reason = trial.invalid_reason.as_deref().unwrap_or("invalid");
            out.push_str(&format!("- `{}` {reason}\n", trial.dir_name()));
        }
    }
    if !any {
        out.push_str("- none\n");
    }
    out.push('\n');
    out
}

fn trial_noisy(trial: &TrialRecord) -> bool {
    trial.noisy
        || trial
            .metrics
            .get("outside_cpu_pct")
            .is_some_and(|v| v >= 50.0)
}

fn stats_lookup<'a>(stats: &'a ScenarioStats) -> impl Fn(&str) -> Option<&'a MetricSummary> + 'a {
    move |id: &str| {
        stats
            .main
            .get(id)
            .or_else(|| stats.wakeups.get(id))
            .or_else(|| stats.pss_groups.get(id))
    }
}

fn main_ids(scenario: &str, stats: &ScenarioStats) -> Vec<String> {
    let mut ids = Vec::new();
    for id in headline_metric_ids_for(scenario) {
        if stats.main.contains_key(*id) {
            ids.push((*id).to_string());
        }
    }
    if stats.main.contains_key("energy_j_net") {
        ids.push("energy_j_net".into());
    }
    for id in stats.main.keys() {
        if !ids.iter().any(|have| have == id) {
            ids.push(id.clone());
        }
    }
    if !scenario.starts_with("S-idle") {
        ids.extend(stats.wakeups.keys().cloned());
    }
    if !scenario.starts_with("S-mcp") {
        ids.extend(stats.pss_groups.keys().cloned());
    }
    ids
}

fn push_table<'a>(
    out: &mut String,
    ids: &[String],
    get: impl Fn(&str) -> Option<&'a MetricSummary>,
) {
    out.push_str(TABLE_HEADER);
    out.push('\n');
    out.push_str(TABLE_RULE);
    out.push('\n');
    for id in ids {
        let Some(summary) = get(id) else {
            continue;
        };
        let label = display_name(id);
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            label.replace('|', "/"),
            fmt_span(summary.a.median, summary.a.min, summary.a.max),
            fmt_span(summary.b.median, summary.b.min, summary.b.max),
            fmt_delta(&summary.paired_delta_pct),
            fmt_noise(summary.noise_pct),
            summary.verdict.as_str(),
        ));
    }
}

fn display_name(id: &str) -> String {
    if let Some(comm) = id.strip_prefix("wakeups_per_s.comm.") {
        return comm.to_string();
    }
    if let Some(group) = id.strip_prefix("pss_mib.group.") {
        return group.to_string();
    }
    id.to_string()
}

fn fmt_span(median: f64, min: f64, max: f64) -> String {
    format!("{} [{}–{}]", fmt_num(median), fmt_num(min), fmt_num(max))
}

fn fmt_delta(delta: &super::stats::DeltaPct) -> String {
    format!(
        "{}% [{}–{}]",
        fmt_num(delta.median),
        fmt_num(delta.min),
        fmt_num(delta.max)
    )
}

fn fmt_noise(noise: Option<f64>) -> String {
    match noise {
        Some(value) => format!("{}%", fmt_num(value)),
        None => "unknown".into(),
    }
}

/// Up to two decimal places, at least one, scientific past 1e7.
pub fn fmt_num(value: f64) -> String {
    if !value.is_finite() {
        return "n/a".into();
    }
    if value.abs() >= 1.0e7 {
        return format!("{value:.3e}");
    }
    let negative = value < 0.0;
    let scaled = (value.abs() * 100.0).round() as i64;
    let whole = scaled / 100;
    let frac = scaled % 100;
    let body = if frac == 0 {
        format!("{whole}.0")
    } else if frac % 10 == 0 {
        format!("{whole}.{}", frac / 10)
    } else {
        format!("{whole}.{frac:02}")
    };
    if negative { format!("-{body}") } else { body }
}

pub fn summary_value(scenario: &str, variant_key: &str, stats: &ScenarioStats) -> Value {
    let mut metrics = serde_json::Map::new();
    for (id, summary) in stats
        .main
        .iter()
        .chain(stats.wakeups.iter())
        .chain(stats.pss_groups.iter())
    {
        metrics.insert(
            id.clone(),
            serde_json::to_value(summary).unwrap_or(Value::Null),
        );
    }
    let mut counters = serde_json::Map::new();
    for (id, summary) in &stats.counters {
        counters.insert(
            id.clone(),
            serde_json::to_value(summary).unwrap_or(Value::Null),
        );
    }
    json!({
        scenario: {
            variant_key: {
                "metrics": metrics,
                "counters": counters
            }
        }
    })
}

pub fn write_summary_and_report(
    run_dir: &Path,
    meta: &super::RunMeta,
    stats: &ScenarioStats,
    trials: &[TrialRecord],
    noise_note: &str,
) -> Result<String> {
    let summary = summary_value(&meta.scenario, &meta.variant_key, stats);
    let summary_path = run_dir.join("summary.json");
    fs::write(
        &summary_path,
        serde_json::to_vec_pretty(&summary).context("encode summary")?,
    )
    .with_context(|| format!("write {}", summary_path.display()))?;
    let report = render_report(meta, stats, trials, noise_note);
    let report_path = run_dir.join("report.md");
    fs::write(&report_path, &report).with_context(|| format!("write {}", report_path.display()))?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_keep_one_decimal_and_an_en_dash_header() {
        assert_eq!(fmt_num(100.0), "100.0");
        assert_eq!(fmt_num(-50.0), "-50.0");
        assert_eq!(fmt_num(3.25), "3.25");
        assert_eq!(fmt_num(0.4), "0.4");
        assert!(TABLE_HEADER.contains("A median [min–max]"));
        assert!(TABLE_HEADER.contains('–'));
    }
}
