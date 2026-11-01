//! Zero-model check of the capture/store/caps path. Run with a credential-free
//! HOME/CLAUDE_CONFIG_DIR and an invalid key; see claude/MCP_VERIFICATION.md.
use std::path::PathBuf;
use std::time::Duration;
use zeron_harness::claude::mcp_capture::{FingerprintInputs, SnapshotStore, probe_caps};
use zeron_harness::{ClaudeHarness, Harness};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("project argument required")?,
    );
    let proxy = PathBuf::from(
        std::env::args_os()
            .nth(2)
            .ok_or("proxy argument required")?,
    );
    // This example is exclusively for the isolated verification procedure.
    if std::env::var("ANTHROPIC_API_KEY").as_deref() != Ok("invalid-zeron-zero-token-verification")
    {
        return Err("use the credential-free verification environment".into());
    }
    let data = PathBuf::from(std::env::var_os("ZERON_DATA_DIR").ok_or("ZERON_DATA_DIR required")?);
    let harness = ClaudeHarness::new();
    let cli = harness.executable_path().ok_or("Claude not installed")?;
    let inputs = FingerprintInputs::for_probe(Some(&project))?;
    let key = inputs.key(&cli)?;
    let store = SnapshotStore::new(&data);
    let commands = harness.commands_for(&project).await?;
    let snapshot = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(snapshot) = store.load(&key) {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    let caps = probe_caps(
        &cli,
        &proxy,
        &["--selftest".into()],
        &snapshot.servers,
        &store,
    )
    .await?;
    println!("cli: {}", cli.display());
    println!(
        "commands: {}; captured servers: {}",
        commands.len(),
        snapshot.servers.len()
    );
    println!("snapshot: {}", store.path(&key).display());
    println!("{}", serde_json::to_string_pretty(&caps)?);
    Ok(())
}
