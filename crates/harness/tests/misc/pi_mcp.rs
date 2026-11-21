//! Exercise the production Pi extension against an actual stdio MCP peer.
//! The optional real-Pi probe uses only an offline provider and isolated settings.
#[tokio::test]
async fn mcp_injection_pi_discovers_calls_cancels_and_reaps() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::process::Command::new("node")
            .arg(root.join("tests/fixtures/pi-mcp-test.mjs"))
            .arg(root.join("src/pi/mcp.mjs"))
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

#[tokio::test]
#[ignore = "requires Pi >= 1.4; set PI_CODING_AGENT_PACKAGE to its installed package directory"]
async fn real_pi_mcp_instructions_images_and_refusals() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        tokio::process::Command::new("node")
            .arg(root.join("tests/fixtures/pi-mcp-live-test.mjs"))
            .arg(root.join("src/pi/mcp.mjs"))
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("real Pi MCP probe must settle and close its peers")
    .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
