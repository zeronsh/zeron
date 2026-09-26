//! Per-run bridge for pi-acp, which accepts but does not implement mcpServers.
use crate::{HarnessError, process::Command, scratch::ScratchDir};

pub(super) fn configure(
    cmd: &mut Command,
    mcp: &zeron_proto::McpServer,
) -> Result<ScratchDir, HarnessError> {
    let scratch = ScratchDir::new("pi-mcp")?;
    let extension = scratch.path().join("zeron-mcp.mjs");
    std::fs::write(&extension, include_str!("pi_mcp.mjs"))?;
    // pi-acp only accepts an executable, not extra argv. This private wrapper
    // adds the extension and otherwise forwards the adapter's RPC/session args.
    #[cfg(unix)]
    let (name, script) = (
        "pi",
        "#!/bin/sh\nexec \"$ZERON_PI_COMMAND\" --extension \"$ZERON_PI_EXTENSION\" \"$@\"\n",
    );
    #[cfg(windows)]
    let (name, script) = (
        "pi.cmd",
        "@echo off\r\n\"%ZERON_PI_COMMAND%\" --extension \"%ZERON_PI_EXTENSION%\" %*\r\n",
    );
    let wrapper = scratch.path().join(name);
    std::fs::write(&wrapper, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700))?;
    }
    cmd.env(
        "ZERON_PI_COMMAND",
        std::env::var_os("PI_ACP_PI_COMMAND").unwrap_or_else(|| name.into()),
    )
    .env("ZERON_PI_EXTENSION", extension)
    .env(
        "ZERON_PI_MCP",
        serde_json::to_string(mcp).expect("MCP config is serializable"),
    )
    .env("PI_ACP_PI_COMMAND", wrapper);
    Ok(scratch)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn mcp_injection_wrapper_preserves_argv_and_is_private() {
        let fixture = tempfile::tempdir().unwrap();
        let fake = fixture.path().join("pi with spaces");
        std::fs::write(&fake, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut cmd = Command::new("sh");
        cmd.args([
            "-c",
            "exec \"$PI_ACP_PI_COMMAND\" --mode rpc --session 'session with spaces'",
        ]);
        let scratch = configure(
            &mut cmd,
            &zeron_proto::McpServer {
                name: "zeron".into(),
                command: "zeron".into(),
                args: vec!["mcp".into()],
                env: Default::default(),
            },
        )
        .unwrap();
        cmd.env("ZERON_PI_COMMAND", fake);
        let output = cmd.output().await.unwrap();
        assert!(output.status.success());
        let args = String::from_utf8(output.stdout).unwrap();
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            vec![
                "--extension",
                scratch.path().join("zeron-mcp.mjs").to_str().unwrap(),
                "--mode",
                "rpc",
                "--session",
                "session with spaces",
            ]
        );
        let path = scratch.path().to_path_buf();
        drop(scratch);
        assert!(!path.exists());
    }
}
