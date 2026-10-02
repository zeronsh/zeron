#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde_json::Value;
use zeron_workflow::testing::FakeHost;
use zeron_workflow::{Limits, RunError, analyze, run_script};

pub fn run_with(
    src: &str,
    args: Value,
    host: &Arc<FakeHost>,
    cancel: Arc<AtomicBool>,
    limits: &Limits,
) -> Result<Value, RunError> {
    let analysis = analyze("wf.star", src).map_err(RunError::Diagnostics)?;
    run_script(
        "wf.star",
        src,
        &args,
        &analysis,
        host.clone(),
        cancel,
        limits,
    )
}

pub fn run(src: &str, args: Value, host: &Arc<FakeHost>) -> Result<Value, RunError> {
    run_with(src, args, host, Arc::default(), &Limits::default())
}

pub fn diagnostics(src: &str) -> Vec<String> {
    match analyze("wf.star", src) {
        Ok(_) => Vec::new(),
        Err(d) => d.iter().map(ToString::to_string).collect(),
    }
}
