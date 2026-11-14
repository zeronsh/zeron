//! The stdio MCP servers a Claude run starts from the user's own
//! configuration — read the way the CLI reads it — so the host can tell
//! them from a background shell among the agent's children
//! ([`crate::process::unexpected_descendants`]).
//!
//! Zeron launches Claude without `--strict-mcp-config`, so the CLI also
//! starts the servers configured in its documented MCP scopes
//! (code.claude.com/docs/en/mcp, /managed-mcp, /claude-directory):
//!
//! - **user**: `mcpServers` at the top of `.claude.json` (in
//!   `$CLAUDE_CONFIG_DIR` when set, else `~/.claude.json`);
//! - **local**: `projects[<project path>].mcpServers` in the same file;
//! - **project**: `.mcp.json` at the project root, which `claude -p` loads
//!   without asking unless a settings file's `disabledMcpjsonServers`
//!   names the server;
//! - **plugins**: each enabled plugin (`enabledPlugins` in the settings
//!   files) installed under `<config>/plugins` (`installed_plugins.json`),
//!   its servers in `.mcp.json` at the plugin root or in
//!   `.claude-plugin/plugin.json`'s `mcpServers`;
//! - **managed**: `managed-mcp.json` in the OS's system location.
//!
//! Only `command` and `args` are taken: an entry's `env` (which may hold
//! secrets) is never read into anything kept or logged. Remote servers
//! (`url`) start no process and are skipped. Files are size-bounded; one
//! missing, too big or malformed adds nothing, which leaves its servers
//! unrecognised — and an unrecognised child holds an idle stop: the safe
//! side.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::process::KnownServer;

/// `.claude.json` holds history and per-project state, and grows: the CLI
/// itself backs it up while under 8 MiB.
const CLAUDE_JSON_MAX: u64 = 16 * 1024 * 1024;
/// Every other file read here.
const FILE_MAX: u64 = 1024 * 1024;
/// The most servers taken from all sources.
const SERVERS_MAX: usize = 512;

#[cfg(target_os = "macos")]
const MANAGED_DIR: &str = "/Library/Application Support/ClaudeCode";
#[cfg(all(unix, not(target_os = "macos")))]
const MANAGED_DIR: &str = "/etc/claude-code";
#[cfg(windows)]
const MANAGED_DIR: &str = r"C:\Program Files\ClaudeCode";

/// The stdio servers a Claude run in `cwd`, with `env` over this process's
/// environment, starts from the user's configuration. Blocking (file
/// reads); call once per run, off the async threads.
pub fn configured_mcp_servers(env: &BTreeMap<String, String>, cwd: &Path) -> Vec<KnownServer> {
    let var = |name: &str| {
        env.get(name)
            .cloned()
            .or_else(|| std::env::var(name).ok())
            .filter(|value| !value.is_empty())
    };
    let home = var("HOME")
        .map(PathBuf::from)
        .or_else(crate::executable::home_dir)
        .unwrap_or_else(crate::executable::home_or_current_dir);
    let config_dir = var("CLAUDE_CONFIG_DIR").map(PathBuf::from);
    let claude_json = match &config_dir {
        Some(dir) => dir.join(".claude.json"),
        None => home.join(".claude.json"),
    };
    let config_dir = config_dir.unwrap_or_else(|| home.join(".claude"));
    let roots = project_roots(cwd);
    let managed = PathBuf::from(MANAGED_DIR);

    let mut found = Vec::new();
    let mut add = |servers: Option<&Value>, extra: &[(&str, String)]| {
        for (command, args) in stdio_servers(servers, &|name| {
            extra
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
                .or_else(|| var(name))
        }) {
            if found.len() < SERVERS_MAX {
                found.push((command, args));
            }
        }
    };

    // User and local scopes.
    let state = read_json(&claude_json, CLAUDE_JSON_MAX);
    if let Some(state) = &state {
        add(state.get("mcpServers"), &[]);
        for root in &roots {
            add(
                state
                    .get("projects")
                    .and_then(|projects| projects.get(root.to_string_lossy().as_ref()))
                    .and_then(|project| project.get("mcpServers")),
                &[],
            );
        }
    }

    // Settings files, lowest precedence first.
    let mut settings: Vec<Value> = Vec::new();
    settings.extend(read_json(&config_dir.join("settings.json"), FILE_MAX));
    for root in roots.iter().rev() {
        settings.extend(read_json(
            &root.join(".claude").join("settings.json"),
            FILE_MAX,
        ));
        settings.extend(read_json(
            &root.join(".claude").join("settings.local.json"),
            FILE_MAX,
        ));
    }
    settings.extend(read_json(&managed.join("managed-settings.json"), FILE_MAX));

    // Project scope, less the servers a settings file turned down.
    let disabled: BTreeSet<String> = settings
        .iter()
        .flat_map(|file| strings(file.get("disabledMcpjsonServers")))
        .collect();
    for root in &roots {
        if let Some(project) = read_json(&root.join(".mcp.json"), FILE_MAX) {
            let kept = project
                .get("mcpServers")
                .and_then(Value::as_object)
                .map(|servers| {
                    Value::Object(
                        servers
                            .iter()
                            .filter(|(name, _)| !disabled.contains(name.as_str()))
                            .map(|(name, server)| (name.clone(), server.clone()))
                            .collect(),
                    )
                });
            add(kept.as_ref(), &[]);
        }
    }

    // Enabled plugins: later settings files win.
    let mut enabled: BTreeMap<String, bool> = BTreeMap::new();
    for file in &settings {
        if let Some(plugins) = file.get("enabledPlugins").and_then(Value::as_object) {
            for (id, on) in plugins {
                if let Some(on) = on.as_bool() {
                    enabled.insert(id.clone(), on);
                }
            }
        }
    }
    let installed = read_json(
        &config_dir.join("plugins").join("installed_plugins.json"),
        FILE_MAX,
    );
    let records = installed
        .as_ref()
        .and_then(|installed| installed.get("plugins"))
        .and_then(Value::as_object);
    for (id, entries) in records.into_iter().flatten() {
        if enabled.get(id) != Some(&true) {
            continue;
        }
        for entry in entries.as_array().into_iter().flatten() {
            // A project-scoped install loads in its own project only.
            if let Some(project) = entry.get("projectPath").and_then(Value::as_str)
                && !roots.iter().any(|root| root == Path::new(project))
            {
                continue;
            }
            let Some(root) = entry.get("installPath").and_then(Value::as_str) else {
                continue;
            };
            let root = PathBuf::from(root);
            let plugin_root = [("CLAUDE_PLUGIN_ROOT", root.to_string_lossy().into_owned())];
            if let Some(file) = read_json(&root.join(".mcp.json"), FILE_MAX) {
                add(file.get("mcpServers").or(Some(&file)), &plugin_root);
            }
            if let Some(manifest) =
                read_json(&root.join(".claude-plugin").join("plugin.json"), FILE_MAX)
            {
                for declared in one_or_many(manifest.get("mcpServers")) {
                    match declared {
                        Value::String(path) => {
                            if let Some(file) = read_json(&root.join(path), FILE_MAX) {
                                add(file.get("mcpServers").or(Some(&file)), &plugin_root);
                            }
                        }
                        servers => add(Some(servers), &plugin_root),
                    }
                }
            }
        }
    }

    // Managed.
    if let Some(file) = read_json(&managed.join("managed-mcp.json"), FILE_MAX) {
        add(file.get("mcpServers"), &[]);
    }
    // (The same file can be reached by two spellings of its directory.)
    let mut seen = BTreeSet::new();
    found.retain(|server| seen.insert(server.clone()));
    found
}

/// The run's directory, then the git repository root above it (where a
/// project's `.mcp.json` and its local-scope entry are kept), each once.
fn project_roots(cwd: &Path) -> Vec<PathBuf> {
    let mut roots = vec![cwd.to_path_buf()];
    if let Ok(real) = cwd.canonicalize()
        && real != cwd
    {
        roots.push(real);
    }
    let mut dir = Some(cwd);
    while let Some(here) = dir {
        if here.join(".git").exists() {
            if !roots.iter().any(|root| root == here) {
                roots.push(here.to_path_buf());
            }
            break;
        }
        dir = here.parent();
    }
    roots
}

fn read_json(path: &Path, max: u64) -> Option<Value> {
    let size = std::fs::metadata(path).ok()?.len();
    if size > max {
        tracing::debug!(target: "zeron_harness::claude", path = %path.display(), size, "MCP config too large to read");
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(value) => Some(value),
        Err(err) => {
            tracing::debug!(target: "zeron_harness::claude", path = %path.display(), error = %err, "MCP config unreadable");
            None
        }
    }
}

fn strings(value: Option<&Value>) -> impl Iterator<Item = String> + '_ {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
}

fn one_or_many(value: Option<&Value>) -> Vec<&Value> {
    match value {
        Some(Value::Array(items)) => items.iter().collect(),
        Some(value) => vec![value],
        None => Vec::new(),
    }
}

/// The stdio entries of a `{ name: server }` map: command and args, each
/// with `${VAR}` / `${VAR:-default}` expanded. Never the entry's `env`.
fn stdio_servers(
    servers: Option<&Value>,
    var: &dyn Fn(&str) -> Option<String>,
) -> Vec<KnownServer> {
    let Some(servers) = servers.and_then(Value::as_object) else {
        return Vec::new();
    };
    servers
        .values()
        .filter_map(|server| {
            let server = server.as_object()?;
            let kind = server
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("stdio");
            if kind != "stdio" || server.contains_key("url") {
                return None;
            }
            let command = expand(server.get("command")?.as_str()?, var);
            let args = server
                .get("args")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|arg| expand(arg, var))
                .collect();
            Some((command, args))
        })
        .collect()
}

/// `${VAR}` and `${VAR:-default}`, as the CLI expands them in MCP config.
fn expand(text: &str, var: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inner = &rest[start + 2..start + end];
        let (name, default) = match inner.split_once(":-") {
            Some((name, default)) => (name, Some(default)),
            None => (inner, None),
        };
        out.push_str(
            &var(name)
                .or_else(|| default.map(str::to_owned))
                .unwrap_or_default(),
        );
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write(path: &Path, value: &Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value.to_string()).unwrap();
    }

    fn servers(config: &Path, cwd: &Path) -> Vec<KnownServer> {
        let env = [
            (
                "CLAUDE_CONFIG_DIR".to_owned(),
                config.to_string_lossy().into_owned(),
            ),
            (
                "HOME".to_owned(),
                config.join("home").to_string_lossy().into_owned(),
            ),
        ]
        .into_iter()
        .collect();
        configured_mcp_servers(&env, cwd)
    }

    fn known(command: &str, args: &[&str]) -> KnownServer {
        (
            command.to_owned(),
            args.iter().map(|arg| arg.to_string()).collect(),
        )
    }

    #[test]
    fn user_local_project_and_plugin_servers_are_read_without_their_env() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let project = dir.path().join("project");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        let plugin = config
            .join("plugins")
            .join("cache")
            .join("m")
            .join("p")
            .join("1.0.0");
        write(
            &config.join(".claude.json"),
            &json!({
                "mcpServers": {
                    "user-one": { "command": "uvx", "args": ["user-server"], "env": { "TOKEN": "secret-value" } },
                    "remote": { "type": "http", "url": "https://example.com/mcp" },
                },
                "projects": {
                    project.to_string_lossy(): { "mcpServers": { "local-one": { "type": "stdio", "command": "${HOME}/bin/local" } } },
                    "/elsewhere": { "mcpServers": { "other": { "command": "nope" } } },
                },
            }),
        );
        write(
            &project.join(".mcp.json"),
            &json!({ "mcpServers": {
                "shared": { "command": "npx", "args": ["-y", "shared-server"] },
                "turned-down": { "command": "never" },
            }}),
        );
        write(
            &project.join(".claude").join("settings.local.json"),
            &json!({ "disabledMcpjsonServers": ["turned-down"] }),
        );
        write(
            &config.join("settings.json"),
            &json!({ "enabledPlugins": { "p@m": true, "off@m": false } }),
        );
        write(
            &config.join("plugins").join("installed_plugins.json"),
            &json!({ "version": 2, "plugins": {
                "p@m": [{ "scope": "user", "installPath": plugin.to_string_lossy() }],
                "off@m": [{ "scope": "user", "installPath": "/nowhere" }],
            }}),
        );
        write(
            &plugin.join(".mcp.json"),
            &json!({ "mcpServers": { "plug": { "command": "${CLAUDE_PLUGIN_ROOT}/bin/server", "args": ["--stdio"] } } }),
        );

        let found = servers(&config, &project);
        let home = config.join("home");
        assert!(found.contains(&known("uvx", &["user-server"])), "{found:?}");
        assert!(
            found.contains(&known(&format!("{}/bin/local", home.display()), &[])),
            "{found:?}"
        );
        assert!(
            found.contains(&known("npx", &["-y", "shared-server"])),
            "{found:?}"
        );
        assert!(
            found.contains(&known(
                &format!("{}/bin/server", plugin.display()),
                &["--stdio"]
            )),
            "{found:?}"
        );
        assert!(
            !found.iter().any(|(command, _)| command == "never"),
            "a disabled .mcp.json server"
        );
        assert!(
            !found.iter().any(|(command, _)| command == "nope"),
            "another project's local server"
        );
        assert_eq!(found.len(), 4, "{found:?}");
        assert!(!format!("{found:?}").contains("secret-value"));
    }

    #[test]
    fn a_missing_or_malformed_config_adds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        assert_eq!(servers(&config, dir.path()), Vec::<KnownServer>::new());
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join(".claude.json"), b"{ not json").unwrap();
        assert_eq!(servers(&config, dir.path()), Vec::<KnownServer>::new());
    }

    #[test]
    fn variables_expand_with_their_defaults() {
        let var = |name: &str| (name == "SET").then(|| "value".to_owned());
        assert_eq!(expand("${SET}/x", &var), "value/x");
        assert_eq!(expand("${UNSET:-fallback}/x", &var), "fallback/x");
        assert_eq!(expand("${UNSET}", &var), "");
        assert_eq!(expand("plain ${", &var), "plain ${");
    }
}
