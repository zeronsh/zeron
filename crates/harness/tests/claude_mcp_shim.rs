//! Explicit native integration: only synthetic config and loopback model traffic.
#[cfg(all(target_os = "linux", feature = "native-fixture"))]
#[test]
#[ignore = "requires ZERON_TEST_CLAUDE and ZERON_TEST_MCP_PROXY; never uses a real profile"]
fn credential_free_launch_interception() {
    let cli = std::env::var_os("ZERON_TEST_CLAUDE").expect("explicit native Claude binary");
    let proxy = std::env::var_os("ZERON_TEST_MCP_PROXY").expect("explicit standalone proxy binary");
    let root = tempfile::Builder::new()
        .prefix("claude-shim-e2e-")
        .tempdir_in(std::env::var_os("TMPDIR").expect("worker scratch TMPDIR"))
        .unwrap();
    let result = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/claude_mcp_shim.py"
        ))
        .arg(root.path())
        .arg(cli)
        .arg(proxy)
        .arg(env!("CARGO_BIN_EXE_harness-claude-shim-fixture"))
        .status()
        .unwrap();
    // Keep the evidence and precise reproduction paths for the validation report.
    let path = root.keep();
    eprintln!("Claude shim evidence: {}", path.display());
    assert!(result.success());
}
