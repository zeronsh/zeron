//! Read-only model discovery using the same resolver and protocol as the picker.
use zeron_harness::{ClaudeHarness, Harness};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let harness = ClaudeHarness::new();
    let context = harness.model_context()?.expect("Claude context");
    println!("binary: {}", context.binary_path.display());
    println!(
        "version: {}",
        context.binary_version.as_deref().unwrap_or("unknown")
    );
    let catalog = harness.model_catalog(true).await?;
    println!("source: {}", catalog.source);
    for model in &catalog.models {
        println!("{} => {}", model.id, model.label);
    }
    println!("models: {}", catalog.models.len());
    Ok(())
}
