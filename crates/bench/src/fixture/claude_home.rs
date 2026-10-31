//! Credential-free synthetic Claude MCP fixture. No user files are read.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use serde_json::json;

const SERVER: &str = r#"'use strict';
const readline = require('readline');
// Touch every page so the resident footprint resembles a Node MCP server.
const ballast = Buffer.alloc(175 * 1024 * 1024, 1);
readline.createInterface({input: process.stdin}).on('line', line => {
  let msg;
  try { msg = JSON.parse(line); } catch (_) { return; }
  if (msg.id === undefined) return;
  let result;
  if (msg.method === 'initialize') result = {protocolVersion: '2024-11-05', capabilities: {tools: {}}, serverInfo: {name: 'zb-synthetic', version: '1'}};
  else if (msg.method === 'tools/list') result = {tools: [{name: 'echo', description: 'Offline benchmark echo', inputSchema: {type: 'object', properties: {}}}]};
  else if (msg.method === 'tools/call' && msg.params?.name === 'echo') result = {content: [{type: 'text', text: 'ok'}]};
  else { process.stdout.write(JSON.stringify({jsonrpc:'2.0', id:msg.id, error:{code:-32601, message:'unknown method'}})+'\n'); return; }
  process.stdout.write(JSON.stringify({jsonrpc:'2.0', id:msg.id, result})+'\n');
});
process.on('exit', () => { if (ballast[0] !== 1) process.exitCode = 1; });
"#;

fn write_new(path: &Path, data: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(data)?;
    Ok(())
}

/// Create a fresh, self-contained fixture below a validated bench root.
/// Existing files are never overwritten or followed.
pub fn setup(root: &Path) -> Result<PathBuf> {
    ensure!(
        root.is_dir() && root.is_absolute(),
        "bench root must exist and be absolute"
    );
    let root = fs::canonicalize(root)?;
    ensure!(
        !root.starts_with("/tmp") && !root.components().any(|c| c.as_os_str() == ".zeron"),
        "unsafe bench root"
    );
    let dir = super::fixture_dir(&root, &["cache", "claude-home"])?;
    let server = dir.join("synthetic-mcp.js");
    let plugin = super::fixture_dir(&root, &["cache", "claude-home", "plugin"])?;
    let metadata =
        super::fixture_dir(&root, &["cache", "claude-home", "plugin", ".claude-plugin"])?;
    let marketplace = super::fixture_dir(&root, &["cache", "claude-home", ".claude-plugin"])?;
    let plugins = super::fixture_dir(&root, &["cache", "claude-home", "plugins"])?;
    let entry = json!({"type":"stdio", "command":"node", "args":[server, "entry"]});
    let plugin_server = json!({"type":"stdio", "command":"node", "args":[server, "plugin"]});
    write_new(&server, SERVER.as_bytes())?;
    write_new(
        &dir.join(".claude.json"),
        &serde_json::to_vec_pretty(&json!({"mcpServers":{"synthetic-entry":entry}}))?,
    )?;
    write_new(
        &dir.join("settings.json"),
        &serde_json::to_vec_pretty(&json!({
            "disableAllHooks":true, "enabledPlugins":{"synthetic@fixture":true}
        }))?,
    )?;
    write_new(
        &metadata.join("plugin.json"),
        &serde_json::to_vec_pretty(&json!({"name":"synthetic", "version":"1.0.0"}))?,
    )?;
    write_new(
        &plugin.join(".mcp.json"),
        &serde_json::to_vec_pretty(&json!({"synthetic":plugin_server}))?,
    )?;
    write_new(
        &marketplace.join("marketplace.json"),
        &serde_json::to_vec_pretty(&json!({
            "name":"fixture", "owner":{"name":"Fixture"}, "plugins":[{"name":"synthetic", "source":"./plugin", "description":"Synthetic benchmark server"}]
        }))?,
    )?;
    write_new(
        &plugins.join("installed_plugins.json"),
        &serde_json::to_vec_pretty(&json!({
            "version":2, "plugins":{"synthetic@fixture":[{"scope":"user", "installPath":plugin, "version":"1.0.0", "installedAt":"2026-01-01T00:00:00Z", "lastUpdated":"2026-01-01T00:00:00Z"}]}
        }))?,
    )?;
    write_new(
        &plugins.join("known_marketplaces.json"),
        &serde_json::to_vec_pretty(&json!({
            "fixture":{"source":{"source":"directory", "path":dir}, "installLocation":dir, "lastUpdated":"2026-01-01T00:00:00Z"}
        }))?,
    )?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::process::Command;

    #[tokio::test]
    async fn synthetic_fixture_is_credential_free_and_speaks_mcp() {
        let root =
            PathBuf::from(std::env::var_os("TMPDIR").expect("isolated TMPDIR required")).join(
                format!("mcp-fixture-{}-{}", std::process::id(), crate::mono_now()),
            );
        fs::create_dir(&root).unwrap();
        let home = setup(&root).unwrap();
        let mut files: Vec<_> = fs::read_dir(&home)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        files.sort();
        assert_eq!(
            files,
            [
                ".claude-plugin",
                ".claude.json",
                "plugin",
                "plugins",
                "settings.json",
                "synthetic-mcp.js"
            ]
        );
        let config: serde_json::Value =
            serde_json::from_slice(&fs::read(home.join(".claude.json")).unwrap()).unwrap();
        assert_eq!(config["mcpServers"].as_object().unwrap().len(), 1);
        let plugin: serde_json::Value =
            serde_json::from_slice(&fs::read(home.join("plugin/.mcp.json")).unwrap()).unwrap();
        assert_ne!(
            config["mcpServers"]["synthetic-entry"]["args"],
            plugin["synthetic"]["args"]
        );
        assert!(!home.join(".credentials.json").exists());
        assert!(setup(&root).is_err());
        let mut command = Command::new("/usr/bin/node");
        command.env_clear().arg(home.join("synthetic-mcp.js"));
        let mut client =
            crate::mcp_client::Client::start(&mut command, Duration::from_secs(10)).unwrap();
        let init = client.initialize().await.unwrap();
        assert_eq!(init["serverInfo"]["name"], "zb-synthetic");
        let list = client.request("tools/list", json!({})).await.unwrap();
        assert_eq!(list["tools"][0]["name"], "echo");
        let (first, cold) = client.timed_call("echo", json!({})).await.unwrap();
        let (second, warm) = client.timed_call("echo", json!({})).await.unwrap();
        assert_eq!(first, second);
        assert!(cold >= 0.0 && warm >= 0.0);
        assert!(client.request("bad/method", json!({})).await.is_err());
        client.stop().await.unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
