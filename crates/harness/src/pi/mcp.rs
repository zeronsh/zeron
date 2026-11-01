//! Pi loads Zeron's delegation tools through a per-run extension.
//!
//! The extension registers the snapshot in `mcp-tools.json` (what `zeron mcp`
//! serves from `tools/list` inside a chat) and spawns the server on the first
//! tool call. `zeron-mcp`'s `pi_static_tool_list_matches_served_tools` fails
//! when that snapshot drifts.
use crate::{HarnessError, process::Command, scratch::ScratchDir};

pub(super) fn configure(
    cmd: &mut Command,
    config: &zeron_proto::McpServer,
) -> Result<ScratchDir, HarnessError> {
    let scratch = ScratchDir::new("pi-mcp")?;
    let extension = scratch.path().join("zeron-mcp.mjs");
    std::fs::write(&extension, include_str!("mcp.mjs"))?;
    // Beside the extension: mcp.mjs loads `./mcp-tools.json` via import.meta.url.
    std::fs::write(
        scratch.path().join("mcp-tools.json"),
        include_str!("mcp-tools.json"),
    )?;
    cmd.arg("--extension").arg(extension).env(
        "ZERON_PI_MCP",
        serde_json::to_string(config).expect("serializable MCP config"),
    );
    Ok(scratch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_writes_extension_and_static_tool_list() {
        let mut cmd = Command::new("pi");
        let config = zeron_proto::McpServer {
            name: "zeron".into(),
            command: "zeron".into(),
            args: vec!["mcp".into()],
            env: [("ZERON_CHAT_ID".into(), "chat-1".into())].into(),
        };
        let scratch = configure(&mut cmd, &config).unwrap();
        let dir = scratch.path();
        assert_eq!(
            std::fs::read_to_string(dir.join("zeron-mcp.mjs")).unwrap(),
            include_str!("mcp.mjs")
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("mcp-tools.json")).unwrap(),
            include_str!("mcp-tools.json")
        );
        let args: Vec<String> = cmd
            .as_std_mut()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![
                "--extension".to_string(),
                dir.join("zeron-mcp.mjs").to_string_lossy().into_owned(),
            ]
        );
        let mcp = cmd
            .as_std_mut()
            .get_envs()
            .find(|(key, _)| *key == "ZERON_PI_MCP")
            .and_then(|(_, value)| value)
            .unwrap()
            .to_str()
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(mcp).unwrap();
        assert_eq!(parsed["name"], "zeron");
        assert_eq!(parsed["env"]["ZERON_CHAT_ID"], "chat-1");
    }
}
