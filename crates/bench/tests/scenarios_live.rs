//! Opt-in live check of one scenario trial against a real engine.
//!
//! ZB_TEST_ENGINE=/abs/zeron-engine ZB_TEST_SCENARIO='S-stream:K=1,turn_ms=8000' \
//!   cargo test --release -p zeron-bench --test scenarios_live -- --ignored --nocapture
#![cfg(unix)]
use std::path::PathBuf;

use anyhow::{Context, Result};
use zeron_bench::ab::{PlannedTrial, Side, Tier};
use zeron_bench::scenario::{RunEnv, build};

#[test]
#[ignore = "needs ZB_TEST_ENGINE; builds fixtures on first use"]
fn one_trial_of_the_named_scenario() -> Result<()> {
    let engine = PathBuf::from(std::env::var_os("ZB_TEST_ENGINE").context("set ZB_TEST_ENGINE")?);
    let name =
        std::env::var("ZB_TEST_SCENARIO").unwrap_or_else(|_| "S-stream:K=1,turn_ms=8000".into());
    let tier = std::env::var("ZB_TEST_TIER")
        .ok()
        .map(|t| Tier::parse(&t))
        .transpose()?
        .unwrap_or(Tier::Quick);
    let root = zeron_bench::default_zb_root();
    let run = format!("live-test-{}", std::process::id());
    let rev = std::env::var("ZB_TEST_FIXTURE_REV")
        .unwrap_or_else(|_| "55ffd4142d1572b26b5b7bca511050f4e7c0e1c1".into());
    let bench = PathBuf::from(env!("CARGO_BIN_EXE_zeron-bench"));
    let env = RunEnv::new(root, run, tier, 1, engine.clone(), engine, bench, rev)?;
    let mut scenario = build(&name, env)?.context("unknown scenario")?;
    let planned = PlannedTrial {
        order_index: 0,
        side: Side::A,
        pair_index: Some(0),
        seed: 1,
    };
    let body = scenario.run_trial(&planned)?;
    println!(
        "valid={} reason={:?} noisy={}",
        body.valid, body.invalid_reason, body.noisy
    );
    for c in &body.checks {
        println!(
            "check {} ok={} {}",
            c.name,
            c.ok,
            c.detail.clone().unwrap_or_default()
        );
    }
    let flat = body.metrics.flatten();
    for (k, v) in &flat {
        if k.starts_with("scenario.")
            || k.starts_with("cpu_s")
            || k.starts_with("latency")
            || k.starts_with("pss_mib.peak")
            || k.starts_with("rpc.")
            || k.starts_with("spawns")
            || k == "instructions_u"
            || k == "write_bytes"
        {
            println!("{k} = {v}");
        }
    }
    anyhow::ensure!(body.checks.iter().all(|c| c.ok), "a check failed");
    Ok(())
}
