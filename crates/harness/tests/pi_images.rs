//! Exercise the production Pi image extension against a stub Pi package.
#[tokio::test]
async fn pi_images_compress_results_and_keep_requests_within_budget() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::process::Command::new("node")
            .arg(root.join("tests/fixtures/pi-images-test.mjs"))
            .arg(root.join("src/pi/images.mjs"))
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("Pi image extension test must finish")
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
