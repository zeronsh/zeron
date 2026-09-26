//! cargo run --release -p zeron-ui --features multi-window-fixture --example multi-window-fixture -- <evidence-dir>
fn main() -> anyhow::Result<()> {
    let output = std::path::PathBuf::from(std::env::args_os().nth(1).expect("evidence directory"));
    std::fs::create_dir_all(&output)?;
    let profile = tempfile::tempdir()?;
    zeron_ui::run_multi_window_fixture(profile.path().into(), output.join("result.txt"))
}
