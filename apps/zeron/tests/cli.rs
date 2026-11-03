//! The `zeron` binary answers with the shared command line (`zeron_cli`):
//! every subcommand's help as the library renders it, and its version. The
//! zgx interface runs the same checks against its own binary
//! (apps/zgx/tests/cli.rs), so the two can't drift apart.

use std::path::PathBuf;
use std::process::Command;

/// The binary under the name people type: clap names it after argv[0].
fn zeron(dir: &std::path::Path) -> PathBuf {
    let link = dir.join("zeron");
    #[cfg(unix)]
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_zeron"), &link).unwrap();
    #[cfg(not(unix))]
    std::fs::copy(env!("CARGO_BIN_EXE_zeron"), &link).unwrap();
    link
}

#[cfg(unix)]
#[test]
fn every_subcommand_s_help_is_the_shared_command_line_s() {
    let dir = tempfile::tempdir().unwrap();
    let zeron = zeron(dir.path());
    let version = env!("CARGO_PKG_VERSION");
    for path in zeron_cli::subcommand_paths(version) {
        let out = Command::new(&zeron)
            .args(&path)
            .arg("--help")
            .env("ZERON_DATA_DIR", dir.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "{path:?}: {out:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim_end(),
            zeron_cli::help_for(version, &path).trim_end(),
            "zeron {} --help",
            path.join(" ")
        );
    }
}

#[test]
fn version_is_zeron_and_the_workspace_s() {
    let out = Command::new(env!("CARGO_BIN_EXE_zeron"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("zeron {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn a_bad_argument_exits_as_clap_does() {
    let out = Command::new(env!("CARGO_BIN_EXE_zeron"))
        .arg("--no-such-flag")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--no-such-flag"));
}
