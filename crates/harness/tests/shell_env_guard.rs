//! Exercise the process-global cache and inherited guards in isolated processes.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn inherited_guards_and_cache() {
    if let Ok(case) = std::env::var("ZERON_TEST_SHELL_ENV_CASE") {
        zeron_harness::shell_env::prewarm();
        let expected = matches!(case.as_str(), "normal" | "empty")
            .then_some(std::ffi::OsStr::new("/probe/bin:/usr/bin:/bin"));
        assert_eq!(zeron_harness::shell_env::login_shell_path(), expected);
        // OnceLock caches both successful and suppressed snapshots.
        assert_eq!(zeron_harness::shell_env::login_shell_path(), expected);
        return;
    }

    for case in ["normal", "empty", "nested", "disabled"] {
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("shell");
        let calls = dir.path().join("calls");
        std::fs::write(
            &shell,
            "#!/bin/sh\n\
             [ \"$ZERON_RESOLVING_ENVIRONMENT\" = 1 ] || exit 1\n\
             echo called >> \"$ZERON_TEST_SHELL_CALLS\"\n\
             echo __ZERON_SHELL_ENV_BEGIN__\n\
             echo PATH=/probe/bin:/usr/bin:/bin\n\
             echo __ZERON_SHELL_ENV_END__\n",
        )
        .unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "inherited_guards_and_cache", "--nocapture"])
            .env("ZERON_TEST_SHELL_ENV_CASE", case)
            .env("ZERON_TEST_SHELL_CALLS", &calls)
            .env("SHELL", &shell)
            .env_remove("ZERON_NO_LOGIN_SHELL")
            .env_remove("ZERON_RESOLVING_ENVIRONMENT")
            .envs(match case {
                "nested" => vec![("ZERON_RESOLVING_ENVIRONMENT", "1")],
                "disabled" => vec![("ZERON_NO_LOGIN_SHELL", "1")],
                "empty" => vec![
                    ("ZERON_RESOLVING_ENVIRONMENT", ""),
                    ("ZERON_NO_LOGIN_SHELL", ""),
                ],
                _ => vec![],
            })
            .status()
            .unwrap();
        assert!(status.success(), "case: {case}");
        if matches!(case, "normal" | "empty") {
            assert_eq!(std::fs::read_to_string(calls).unwrap(), "called\n");
        } else {
            assert!(!calls.exists(), "{case} must not start a shell");
        }
    }
}
