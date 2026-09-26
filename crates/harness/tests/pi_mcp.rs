//! Exercise the production Pi extension against an actual stdio MCP peer.
#[tokio::test]
async fn mcp_injection_pi_discovers_calls_cancels_and_reaps() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::process::Command::new("node")
            .arg(root.join("tests/fixtures/pi-mcp-test.mjs"))
            .arg(root.join("src/acp/pi_mcp.mjs"))
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
