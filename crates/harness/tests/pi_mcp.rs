//! The Pi extension registers its static tool list at session start and
//! spawns the stdio peer on the first call.
#[tokio::test]
async fn mcp_injection_pi_discovers_calls_cancels_and_reaps() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = std::path::PathBuf::from(std::env::var("HOME").unwrap())
        .join("Documents/dev/tmp/zeron-perf/scratch/h12-pi-lazy-mcp");
    std::fs::create_dir_all(&scratch).unwrap();
    let mark = scratch.join(format!("pi-mcp-spawns-{}", std::process::id()));
    let _ = std::fs::remove_file(&mark);
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::process::Command::new("node")
            .arg(root.join("tests/fixtures/pi-mcp-test.mjs"))
            .arg(root.join("src/pi/mcp.mjs"))
            .env("ZERON_PI_SPAWN_MARK", &mark)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("Pi MCP bridge must close all subprocesses")
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
