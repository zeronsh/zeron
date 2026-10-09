use std::path::Path;
use std::process::Command;

fn run(target: &Path, data_dir: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_zeron"))
        .arg(target)
        .env("ZERON_DATA_DIR", data_dir)
        .env("ZERON_IPC_PORT", "0")
        .output()
        .expect("zeron process")
}

#[test]
fn a_missing_or_file_target_fails_before_the_app_starts() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "x").unwrap();
    for (target, expected) in [
        (dir.path().join("missing"), "does not exist"),
        (file, "is not a directory"),
    ] {
        let output = run(&target, dir.path());
        assert!(!output.status.success(), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "{stderr}");
        assert!(!dir.path().join("headed.lock").exists());
    }
}
