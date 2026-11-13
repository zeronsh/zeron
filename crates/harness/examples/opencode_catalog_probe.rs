//! Live probe, read-only: the Agent choices of OpenCode's model catalog as
//! the engine reads them, printed whenever they change, for a while. An agent
//! added (or a provider signed into) in the TUI shows up here without a
//! restart.
//!
//!     cargo run -p zeron-harness --example opencode_catalog_probe -- [seconds]

use zeron_harness::{Harness, OpencodeHarness};

#[tokio::main]
async fn main() {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let harness = OpencodeHarness::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let mut last: Option<Vec<String>> = None;
    while tokio::time::Instant::now() < deadline {
        match harness.model_catalog(false).await {
            Ok(catalog) => {
                let agents: Vec<String> = catalog
                    .models
                    .first()
                    .and_then(|model| model.options.iter().find(|option| option.id == "agent"))
                    .map(|option| option.choices.iter().map(|c| c.label.clone()).collect())
                    .unwrap_or_default();
                if last.as_ref() != Some(&agents) {
                    println!(
                        "{} models; agents: {}",
                        catalog.models.len(),
                        agents.join(", ")
                    );
                    last = Some(agents);
                }
            }
            Err(error) => println!("catalog error: {error}"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
