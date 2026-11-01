//! Isolated CLI launch/capture helper for claude_mcp_shim.rs. No profile lookup.
#[cfg(unix)]
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    zeron_harness::claude::shim_fixture(
        std::path::Path::new(&args[0]),
        std::path::Path::new(&args[1]),
        args[2].to_str().unwrap(),
    )
    .await
}

#[cfg(not(unix))]
fn main() {}
