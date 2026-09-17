//! Keep this as a single-test binary: it changes process environment variables.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

use zeron_engine::{AgentAccounts, AgentAccountsConfig};
use zeron_proto::HarnessId;

#[tokio::test]
async fn codex_login_resolves_override_and_adds_its_directory_to_child_path() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("custom-bin");
    std::fs::create_dir(&bin).unwrap();
    // Model an npm shim whose interpreter is installed beside the CLI and
    // absent from the daemon's PATH.
    let exe = bin.join("custom-codex");
    std::fs::write(&exe, "#!/usr/bin/env codex-test-runtime\n").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let runtime = bin.join("codex-test-runtime");
    std::fs::write(
        &runtime,
        "#!/bin/sh\n[ \"$2\" = login ] || exit 1\n[ -d \"$CODEX_HOME\" ] || exit 2\nprintf '%s\\n' 'https://auth.openai.com/authorize?test=resolver'\n",
    )
    .unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();

    // SAFETY: this binary has one test and no other environment users yet.
    unsafe {
        std::env::set_var("CODEX_EXECUTABLE", &exe);
        std::env::set_var("PATH", "/usr/bin:/bin");
        std::env::set_var("ZERON_NO_LOGIN_SHELL", "1");
    }
    let accounts = AgentAccounts::new(AgentAccountsConfig::detect(dir.path()));
    let login = accounts.start_login(HarnessId::Codex).await.unwrap();
    assert_eq!(login.url, "https://auth.openai.com/authorize?test=resolver");
    accounts.cancel_login(&login.login_id);
}
