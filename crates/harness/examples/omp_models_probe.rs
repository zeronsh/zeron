//! Live probe: Oh My Pi model discovery through the native driver — runs
//! `omp models --json` and prints the picker catalog with per-model thinking
//! levels. Needs `omp` on PATH (or OMP_EXECUTABLE); makes no model requests.
//!
//!     cargo run -p zeron-harness --example omp_models_probe

use zeron_harness::{Harness, OmpHarness};

#[tokio::main]
async fn main() {
    let start = std::time::Instant::now();
    match OmpHarness::new().models().await {
        Ok(models) => {
            for m in &models {
                eprintln!("{:48} {:32} {:?}", m.id, m.label, m.reasoning_levels);
                eprintln!("  options: {:?}", m.options);
            }
            eprintln!("--- {} models in {:?}", models.len(), start.elapsed());
        }
        Err(e) => {
            eprintln!("discovery failed: {e}");
            std::process::exit(1);
        }
    }
}
