//! Per-process Pi extensions: image compaction for every run, plus the Zeron
//! delegation tools when configured. No global settings are modified.
use crate::{HarnessError, process::Command, scratch::ScratchDir};

pub(super) fn configure(
    cmd: &mut Command,
    config: Option<&zeron_proto::McpServer>,
) -> Result<ScratchDir, HarnessError> {
    let scratch = ScratchDir::new("pi-ext")?;
    let images = scratch.path().join("zeron-images.mjs");
    std::fs::write(&images, include_str!("images.mjs"))?;
    cmd.arg("--extension").arg(images);
    if let Some(config) = config {
        let extension = scratch.path().join("zeron-mcp.mjs");
        std::fs::write(&extension, include_str!("mcp.mjs"))?;
        cmd.arg("--extension").arg(extension).env(
            "ZERON_PI_MCP",
            serde_json::to_string(config).expect("serializable MCP config"),
        );
    }
    Ok(scratch)
}
