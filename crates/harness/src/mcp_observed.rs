//! Servers Settings can list without launching a harness.
//!
//! Claude rows come from the newest fresh snapshot of each fingerprint.
//! Codex rows come from the last cached `config/read`, or from `config.toml`
//! when nothing is cached. Computed on request. No background scan.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use zeron_proto::HarnessId;

use crate::claude::mcp_capture::{self, ClaudeMcpServer, SnapshotStore};
use crate::mcp_policy::{self, McpClass, McpServerFact};

const CACHE_VERSION: u64 = 1;
const KEPT_ROW_KEYS: &[&str] = &[
    "command",
    "args",
    "enabled",
    "capabilities",
    "experimental",
    "channelCapabilities",
    "transport",
];

/// One server a harness config currently names. Running state is attached
/// by the engine from this data dir's proxy markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedMcpServer {
    pub harness: HarnessId,
    pub name: String,
    pub class: McpClass,
    pub scope: Option<String>,
}

/// Fresh Claude snapshots plus Codex config for `data_dir`.
/// `codex_config` is the cheap `config.toml` read used only when no
/// `config/read` result has been cached. `None` skips that read.
pub fn discovered_servers(data_dir: &Path, codex_config: Option<&Path>) -> Vec<ObservedMcpServer> {
    let mut servers = claude_servers(data_dir);
    servers.extend(codex_servers(data_dir, codex_config));
    servers.sort_by(|left, right| {
        left.harness
            .as_str()
            .cmp(right.harness.as_str())
            .then_with(|| left.name.cmp(&right.name))
    });
    servers
}

/// Cache a successful `config/read` body under `data_dir`. Secrets are
/// dropped. A body without an `mcp_servers` object writes nothing.
/// A cache file that would leave the data dir is refused.
pub fn remember_codex_config_read(data_dir: &Path, config: &Value) -> io::Result<()> {
    let Some(servers) = config
        .pointer("/config/mcp_servers")
        .and_then(Value::as_object)
    else {
        return Ok(());
    };
    let data_dir = data_dir.canonicalize()?;
    let dir = data_dir.join("codex-mcp");
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&dir)?;
    let dir = dir.canonicalize()?;
    if !dir.starts_with(&data_dir) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "codex mcp cache escapes the data dir",
        ));
    }
    let body = json!({
        "version": CACHE_VERSION,
        "mcp_servers": strip_servers(servers),
    });
    atomic_private(&dir.join("config-read.json"), body.to_string().as_bytes())
}

/// `$CODEX_HOME/config.toml` when that variable is set, otherwise
/// `$HOME/.codex/config.toml`.
pub fn default_codex_config() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(home).join("config.toml"));
    }
    let home = std::env::var_os("HOME").filter(|value| !value.is_empty())?;
    Some(PathBuf::from(home).join(".codex").join("config.toml"))
}

fn claude_servers(data_dir: &Path) -> Vec<ObservedMcpServer> {
    let snapshots = SnapshotStore::new(data_dir).latest_fresh();
    let mut ranked: Vec<&mcp_capture::ClaudeMcpSnapshot> = snapshots.iter().collect();
    ranked.sort_by(|left, right| {
        right
            .captured_at_ms
            .cmp(&left.captured_at_ms)
            .then_with(|| right.key.digest().cmp(&left.key.digest()))
    });
    let mut servers = Vec::new();
    let mut seen = BTreeSet::new();
    for snapshot in ranked {
        for row in &snapshot.servers {
            if row.name.is_empty() || !seen.insert(row.name.clone()) {
                continue;
            }
            servers.push(observe_claude(row));
        }
    }
    servers
}

fn observe_claude(row: &ClaudeMcpServer) -> ObservedMcpServer {
    let command = row
        .config
        .as_ref()
        .and_then(|config| config.get("command"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let transport = row
        .transport
        .as_deref()
        .filter(|transport| !transport.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if command.is_empty() {
                String::new()
            } else {
                "stdio".to_owned()
            }
        });
    let channel = row.extra.iter().any(|(key, value)| {
        (key == "channelCapabilities" || key == "capabilities")
            && mcp_policy::value_declares_channel(value)
    });
    let fact = McpServerFact {
        name: row.name.clone(),
        transport,
        command,
        npx_resolved: false,
        channel,
        scope: row.scope.clone().unwrap_or_default(),
    };
    ObservedMcpServer {
        harness: HarnessId::ClaudeCode,
        name: row.name.clone(),
        class: mcp_policy::classify(&fact),
        scope: row
            .scope
            .clone()
            .filter(|scope| !scope.is_empty())
            .or_else(|| row.source.clone().filter(|source| !source.is_empty())),
    }
}

fn codex_servers(data_dir: &Path, codex_config: Option<&Path>) -> Vec<ObservedMcpServer> {
    let Some(servers) = read_cached(data_dir).or_else(|| codex_config.and_then(read_toml_servers))
    else {
        return Vec::new();
    };
    let mut names: Vec<&String> = servers.keys().collect();
    names.sort();
    names
        .into_iter()
        .filter(|name| !name.is_empty())
        .filter_map(|name| {
            let row = servers.get(name)?.as_object()?;
            let fact = crate::codex::config_server_fact(name, row);
            Some(ObservedMcpServer {
                harness: HarnessId::Codex,
                name: name.clone(),
                class: mcp_policy::classify(&fact),
                scope: None,
            })
        })
        .collect()
}

/// `Some` is a cache hit, including an empty server map. `None` means
/// missing, unreadable, the wrong version, or a file outside the data dir.
fn read_cached(data_dir: &Path) -> Option<Map<String, Value>> {
    let data_dir = data_dir.canonicalize().ok()?;
    let path = data_dir.join("codex-mcp").join("config-read.json");
    let path = path.canonicalize().ok()?;
    if !path.starts_with(&data_dir) {
        return None;
    }
    let value: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if value.get("version").and_then(Value::as_u64) != Some(CACHE_VERSION) {
        return None;
    }
    value.get("mcp_servers")?.as_object().cloned()
}

fn read_toml_servers(path: &Path) -> Option<Map<String, Value>> {
    let text = std::fs::read_to_string(path).ok()?;
    let parsed: toml::Value = toml::from_str(&text).ok()?;
    let table = parsed.get("mcp_servers")?.as_table()?.clone();
    let json = serde_json::to_value(toml::Value::Table(table)).ok()?;
    json.as_object().cloned()
}

fn strip_servers(servers: &Map<String, Value>) -> Map<String, Value> {
    let mut kept = Map::new();
    for (name, row) in servers {
        let Some(row) = row.as_object() else {
            continue;
        };
        let mut stripped = Map::new();
        for key in KEPT_ROW_KEYS {
            if let Some(value) = row.get(*key) {
                stripped.insert((*key).to_owned(), value.clone());
            }
        }
        if row.contains_key("url") {
            stripped.insert("url".to_owned(), Value::Bool(true));
        }
        kept.insert(name.clone(), Value::Object(stripped));
    }
    kept
}

fn atomic_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing cache directory"))?;
    let tmp = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::mcp_capture::{
        CaptureKey, ClaudeMcpSnapshot, FingerprintInputs, fingerprint, fresh_capture_key,
        platform_managed_dir,
    };
    use std::collections::BTreeMap;

    fn server(
        name: &str,
        transport: &str,
        command: Option<&str>,
        scope: Option<&str>,
        source: Option<&str>,
    ) -> ClaudeMcpServer {
        let mut config = Map::new();
        if let Some(command) = command {
            config.insert("command".into(), json!(command));
            config.insert("args".into(), json!([]));
        }
        ClaudeMcpServer {
            name: name.into(),
            status: "connected".into(),
            config: (!config.is_empty()).then(|| Value::Object(config)),
            scope: scope.map(str::to_owned),
            source: source.map(str::to_owned),
            transport: (!transport.is_empty()).then(|| transport.to_owned()),
            tools: None,
            extra: BTreeMap::new(),
        }
    }

    fn project(root: &Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir(dir.join(".git")).unwrap();
        dir
    }

    fn write_raw(store: &SnapshotStore, snapshot: &ClaudeMcpSnapshot) {
        let path = store.path(&snapshot.key);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(snapshot).unwrap()).unwrap();
    }

    #[test]
    fn mcp_observed_lists_latest_fresh_snapshots_for_this_data_dir_only() {
        let home = tempfile::tempdir().unwrap();
        let config_root = home.path().join("config");
        std::fs::create_dir_all(&config_root).unwrap();
        std::fs::write(config_root.join(".claude.json"), "{}").unwrap();
        let project_dir = project(home.path(), "project");
        let key = fresh_capture_key(&config_root, &project_dir, "cli-a").unwrap();
        let mut older = ClaudeMcpSnapshot::new(
            key.clone(),
            vec![server("old-server", "stdio", Some("reins"), None, None)],
        );
        older.captured_at_ms = 1;
        let mut newer_key = key.clone();
        newer_key.cli_binary_identity = "cli-b".into();
        let mut newer = ClaudeMcpSnapshot::new(
            newer_key,
            vec![server(
                "playwright",
                "stdio",
                Some("npx"),
                Some("user"),
                None,
            )],
        );
        newer.captured_at_ms = 2;
        let other_project = project(home.path(), "other");
        std::fs::write(other_project.join(".mcp.json"), "{\"servers\":{}}").unwrap();
        let other_key = fresh_capture_key(&config_root, &other_project, "cli-a").unwrap();
        assert_ne!(other_key.fingerprint, key.fingerprint);
        let other = ClaudeMcpSnapshot::new(
            other_key,
            vec![server(
                "reins",
                "stdio",
                Some("reins"),
                None,
                Some("plugin"),
            )],
        );
        let data = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(data.path());
        store.save(&older).unwrap();
        store.save(&newer).unwrap();
        store.save(&other).unwrap();

        let mut stale_key = key.clone();
        stale_key.fingerprint = "stale".into();
        stale_key.cli_binary_identity = "stale-cli".into();
        write_raw(
            &store,
            &ClaudeMcpSnapshot::new(
                stale_key,
                vec![server("stale-server", "stdio", Some("reins"), None, None)],
            ),
        );
        let mut pending = ClaudeMcpSnapshot::new(
            fresh_capture_key(&config_root, &project(home.path(), "pending"), "cli-a").unwrap(),
            vec![server("pending-server", "stdio", Some("reins"), None, None)],
        );
        pending.servers[0].status = "pending".into();
        write_raw(&store, &pending);
        let mut ancient = ClaudeMcpSnapshot::new(
            fresh_capture_key(&config_root, &project(home.path(), "ancient"), "cli-a").unwrap(),
            vec![server("ancient-server", "stdio", Some("reins"), None, None)],
        );
        ancient.version = 2;
        write_raw(&store, &ancient);
        let mismatch = ClaudeMcpSnapshot::new(
            fresh_capture_key(&config_root, &project(home.path(), "mismatch"), "cli-a").unwrap(),
            vec![server(
                "mismatched-server",
                "stdio",
                Some("reins"),
                None,
                None,
            )],
        );
        let wrong_name = store
            .path(&mismatch.key)
            .with_file_name("not-the-digest.json");
        std::fs::write(wrong_name, serde_json::to_vec(&mismatch).unwrap()).unwrap();

        let listed = discovered_servers(data.path(), None);
        let names: Vec<_> = listed.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, vec!["playwright", "reins"]);
        let playwright = listed.iter().find(|row| row.name == "playwright").unwrap();
        assert_eq!(playwright.harness, HarnessId::ClaudeCode);
        assert_eq!(playwright.class, McpClass::HeavyRuntime);
        assert_eq!(playwright.scope.as_deref(), Some("user"));
        let reins = listed.iter().find(|row| row.name == "reins").unwrap();
        assert_eq!(reins.class, McpClass::NativeSmall);
        assert_eq!(reins.scope.as_deref(), Some("plugin"));

        let elsewhere = tempfile::tempdir().unwrap();
        assert!(discovered_servers(elsewhere.path(), None).is_empty());

        let claude_home = tempfile::tempdir().unwrap();
        let config_root = claude_home.path().join(".claude");
        std::fs::create_dir_all(&config_root).unwrap();
        let project_dir = project(claude_home.path(), "project");
        std::fs::write(
            claude_home.path().join(".claude.json"),
            r#"{"mcpServers":{"home-layout":{"command":"reins"}}}"#,
        )
        .unwrap();
        let inputs = FingerprintInputs {
            config_root: config_root.canonicalize().unwrap(),
            config_file: claude_home
                .path()
                .canonicalize()
                .unwrap()
                .join(".claude.json"),
            project_dir: project_dir.canonicalize().unwrap(),
            managed_dirs: vec![platform_managed_dir()],
        };
        let layout_one = FingerprintInputs {
            config_file: inputs.config_root.join(".claude.json"),
            ..inputs.clone()
        };
        let home_fingerprint = fingerprint(&inputs).unwrap();
        assert_ne!(
            home_fingerprint,
            fingerprint(&layout_one).unwrap(),
            "the home file must not hash like the missing layout"
        );
        let home_key = CaptureKey {
            config_root: inputs.config_root.clone(),
            project_dir: inputs.project_dir.clone(),
            fingerprint: home_fingerprint,
            cli_binary_identity: "cli".into(),
        };
        SnapshotStore::new(data.path())
            .save(&ClaudeMcpSnapshot::new(
                home_key,
                vec![server("home-layout", "stdio", Some("reins"), None, None)],
            ))
            .unwrap();
        assert!(
            discovered_servers(data.path(), None)
                .iter()
                .any(|row| row.name == "home-layout")
        );
    }

    #[cfg(unix)]
    #[test]
    fn mcp_observed_ignores_a_snapshot_store_that_escapes_the_data_dir() {
        let ours = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let config_root = other.path().join("config");
        std::fs::create_dir_all(&config_root).unwrap();
        std::fs::write(config_root.join(".claude.json"), "{}").unwrap();
        let project_dir = project(other.path(), "project");
        let key = fresh_capture_key(&config_root, &project_dir, "cli").unwrap();
        SnapshotStore::new(other.path())
            .save(&ClaudeMcpSnapshot::new(
                key,
                vec![server("foreign", "stdio", Some("reins"), None, None)],
            ))
            .unwrap();
        std::os::unix::fs::symlink(
            other.path().join("claude-mcp"),
            ours.path().join("claude-mcp"),
        )
        .unwrap();
        assert!(discovered_servers(ours.path(), None).is_empty());
    }

    #[test]
    fn mcp_codex_cache_beats_a_cheap_config_read() {
        let data = tempfile::tempdir().unwrap();
        let config = json!({"config": {"mcp_servers": {
            "notes": {
                "command": "npx",
                "args": ["notes"],
                "env": {"TOKEN": "secret-token"},
                "env_vars": ["SECRET"],
                "headers": {"Authorization": "Bearer secret-token"},
                "url": "https://example.test/?token=secret-token"
            },
            "docs": {"url": "https://example.test/secret-token"},
            "bridge": {
                "command": "node",
                "capabilities": {"experimental": {"claude/channel": {}}}
            },
            "parked": {"command": "node", "enabled": false}
        }}});
        remember_codex_config_read(data.path(), &config).unwrap();
        let path = data.path().join("codex-mcp").join("config-read.json");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("secret-token"), "{text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let toml_path = data.path().join("config.toml");
        std::fs::write(&toml_path, "[mcp_servers.toml_only]\ncommand = \"reins\"\n").unwrap();
        let listed = discovered_servers(data.path(), Some(&toml_path));
        let class = |name: &str| listed.iter().find(|row| row.name == name).unwrap().class;
        assert_eq!(class("notes"), McpClass::HeavyRuntime);
        assert_eq!(class("docs"), McpClass::Remote);
        assert_eq!(class("bridge"), McpClass::Channel);
        assert_eq!(class("parked"), McpClass::HeavyRuntime);
        assert!(listed.iter().all(|row| row.name != "toml_only"));
        assert!(listed.iter().all(|row| row.scope.is_none()));

        remember_codex_config_read(data.path(), &json!({"config": {"mcp_servers": {}}})).unwrap();
        assert!(
            discovered_servers(data.path(), Some(&toml_path))
                .iter()
                .all(|row| row.harness != HarnessId::Codex)
        );
        let bare = tempfile::tempdir().unwrap();
        remember_codex_config_read(
            bare.path(),
            &json!({"config": {"mcp_servers": "secret-token"}}),
        )
        .unwrap();
        assert!(
            !bare
                .path()
                .join("codex-mcp")
                .join("config-read.json")
                .exists()
        );
    }

    #[test]
    fn mcp_codex_cheap_read_classifies_stdio_remote_and_native() {
        let data = tempfile::tempdir().unwrap();
        let toml_path = data.path().join("config.toml");
        std::fs::write(
            &toml_path,
            r#"
[mcp_servers.notes]
command = "npx"

[mcp_servers.docs]
url = "https://example.test"

[mcp_servers.reins]
command = "reins"

[mcp_servers.parked]
command = "node"
enabled = false
"#,
        )
        .unwrap();
        let listed = discovered_servers(data.path(), Some(&toml_path));
        let class = |name: &str| listed.iter().find(|row| row.name == name).unwrap().class;
        assert_eq!(class("notes"), McpClass::HeavyRuntime);
        assert_eq!(class("docs"), McpClass::Remote);
        assert_eq!(class("reins"), McpClass::NativeSmall);
        assert_eq!(class("parked"), McpClass::HeavyRuntime);
        assert!(listed.iter().all(|row| row.harness == HarnessId::Codex));
    }
}
