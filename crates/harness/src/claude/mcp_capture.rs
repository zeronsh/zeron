//! Claude owns the MCP merge. Files invalidate captures; they never reconstruct it.
//! Missing/partial status or unproved capabilities must leave WU5 on the legacy path.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::process::{Command, Stdio};

const VERSION: u32 = 1;
const STATUS_ID: &str = "zeron-mcp-status";
const STATUS_TIMEOUT: Duration = Duration::from_secs(25);
const MCP_KEYS: &[&str] = &[
    "mcpServers",
    "enabledMcpjsonServers",
    "disabledMcpjsonServers",
    "enableAllProjectMcpServers",
    "disabledMcpServers",
    "enabledMcpServers",
];
const SETTINGS_KEYS: &[&str] = &[
    "enabledPlugins",
    "enableAllProjectMcpServers",
    "enabledMcpjsonServers",
    "disabledMcpjsonServers",
    "allowedMcpServers",
    "deniedMcpServers",
    "extraKnownMarketplaces",
    "channelsEnabled",
    "allowedChannelPlugins",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaudeMcpServer {
    pub name: String,
    pub status: String,
    pub config: Option<Value>,
    pub scope: Option<String>,
    pub source: Option<String>,
    pub transport: Option<String>,
    pub tools: Option<Vec<Value>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ClaudeMcpServer {
    /// 2.1.314 deliberately omits env even when configured. Absence is NOT {}.
    /// Do not fill it from files: plugin/inline sources may be undiscoverable.
    pub fn stdio_config_complete(&self) -> bool {
        let Some(config) = &self.config else {
            return false;
        };
        self.transport.as_deref() == Some("stdio")
            && config
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
            && config
                .get("args")
                .and_then(Value::as_array)
                .is_some_and(|args| args.iter().all(Value::is_string))
            && config
                .get("env")
                .and_then(Value::as_object)
                .is_some_and(|env| env.values().all(Value::is_string))
    }
}

/// Accept the full control frame, without silently turning an error into an empty fleet.
/// Unknown row metadata is retained for future transports/channel declarations.
pub fn parse_status(frame: &Value) -> io::Result<Vec<ClaudeMcpServer>> {
    if frame.get("type").and_then(Value::as_str) != Some("control_response")
        || frame.pointer("/response/subtype").and_then(Value::as_str) != Some("success")
    {
        return Err(invalid("MCP status control request failed"));
    }
    let rows = frame
        .pointer("/response/response/mcpServers")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("MCP status has no server list"))?;
    let rows: Vec<ClaudeMcpServer> = serde_json::from_value(Value::Array(rows.clone()))
        .map_err(|_| invalid("invalid MCP status server list"))?;
    let mut names = BTreeSet::new();
    if rows
        .iter()
        .any(|r| r.name.is_empty() || r.status.is_empty() || !names.insert(&r.name))
    {
        return Err(invalid("empty or duplicate MCP status server name/status"));
    }
    Ok(rows)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureKey {
    pub config_root: PathBuf,
    pub project_dir: PathBuf,
    pub fingerprint: String,
    pub cli_binary_identity: String,
}

impl CaptureKey {
    pub fn digest(&self) -> String {
        digest(&serde_json::to_value(self).expect("serializable capture key"))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaudeMcpSnapshot {
    pub version: u32,
    pub key: CaptureKey,
    pub captured_at_ms: u64,
    pub servers: Vec<ClaudeMcpServer>,
}

impl ClaudeMcpSnapshot {
    pub fn new(key: CaptureKey, servers: Vec<ClaudeMcpServer>) -> Self {
        Self {
            version: VERSION,
            key,
            captured_at_ms: now_ms(),
            servers,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaudeMcpCaps {
    pub version: u32,
    pub cli_binary_identity: String,
    pub strict_drops_plugins: bool,
    /// None means no authenticated baseline connector was observed, not false.
    pub strict_keeps_claudeai: Option<bool>,
    pub status_has_config: bool,
    pub status_preserves_env: bool,
    pub probed_at_ms: u64,
}

impl ClaudeMcpCaps {
    pub fn permits_rewrite(&self) -> bool {
        self.version == VERSION
            && self.strict_drops_plugins
            && self.status_has_config
            && self.status_preserves_env
    }

    fn from_rows(identity: String, baseline: &[ClaudeMcpServer], rows: &[ClaudeMcpServer]) -> Self {
        let test = rows.iter().find(|row| {
            row.name == "zeron_caps"
                && row.status == "connected"
                && row.tools.as_ref().is_some_and(|tools| {
                    tools.iter().any(|tool| {
                        tool.get("name").and_then(Value::as_str) == Some("zeron_proxy_selftest")
                    })
                })
        });
        Self {
            version: VERSION,
            cli_binary_identity: identity,
            // Also reject unexpected user/project/local/managed/dynamic rows.
            strict_drops_plugins: rows.iter().all(|row| {
                row.source.as_deref() != Some("plugin")
                    && (row.name == "zeron_caps" || row.scope.as_deref() == Some("claudeai"))
            }),
            strict_keeps_claudeai: baseline
                .iter()
                .any(|r| r.scope.as_deref() == Some("claudeai"))
                .then(|| rows.iter().any(|r| r.scope.as_deref() == Some("claudeai"))),
            status_has_config: test.is_some_and(|r| {
                r.config
                    .as_ref()
                    .and_then(|c| c.get("command"))
                    .and_then(Value::as_str)
                    .is_some()
            }),
            status_preserves_env: test.is_some_and(|r| {
                r.stdio_config_complete()
                    && r.config
                        .as_ref()
                        .and_then(|c| c.pointer("/env/ZERON_MCP_CAPS_SENTINEL"))
                        == Some(&json!("zeron-caps"))
            }),
            probed_at_ms: now_ms(),
        }
    }
}

pub struct SnapshotStore {
    root: PathBuf,
}

impl SnapshotStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("claude-mcp"),
        }
    }
    pub fn path(&self, key: &CaptureKey) -> PathBuf {
        self.root.join(format!("{}.json", key.digest()))
    }
    pub fn load(&self, key: &CaptureKey) -> Option<ClaudeMcpSnapshot> {
        let snapshot: ClaudeMcpSnapshot =
            serde_json::from_slice(&std::fs::read(self.path(key)).ok()?).ok()?;
        (snapshot.version == VERSION
            && snapshot.key == *key
            && snapshot.servers.iter().all(|r| !pending(r)))
        .then_some(snapshot)
    }
    pub fn save(&self, snapshot: &ClaudeMcpSnapshot) -> io::Result<()> {
        if snapshot.version != VERSION || snapshot.servers.iter().any(pending) {
            return Err(invalid("incomplete MCP capture"));
        }
        atomic_private_json(&self.path(&snapshot.key), snapshot)
    }
    pub fn invalidate(&self, key: &CaptureKey) -> io::Result<()> {
        match std::fs::remove_file(self.path(key)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
    pub fn load_caps(&self, identity: &str) -> Option<ClaudeMcpCaps> {
        let caps: ClaudeMcpCaps =
            serde_json::from_slice(&std::fs::read(self.root.join("caps.json")).ok()?).ok()?;
        (caps.version == VERSION && caps.cli_binary_identity == identity).then_some(caps)
    }
    pub fn save_caps(&self, caps: &ClaudeMcpCaps) -> io::Result<()> {
        atomic_private_json(&self.root.join("caps.json"), caps)
    }

    /// Newest fresh snapshot for each fingerprint under this data dir.
    /// A snapshot is fresh when its stored fingerprint still matches a
    /// candidate layout of its config root and project. Pending rows, a
    /// mismatched digest, a bad version, and a store that escapes the data
    /// dir are omitted. Read errors fail closed.
    pub fn latest_fresh(&self) -> Vec<ClaudeMcpSnapshot> {
        let Some(data_dir) = self.root.parent() else {
            return Vec::new();
        };
        let (Ok(data_dir), Ok(root)) = (data_dir.canonicalize(), self.root.canonicalize()) else {
            return Vec::new();
        };
        if !root.starts_with(&data_dir) {
            return Vec::new();
        }
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Vec::new();
        };
        let mut fingerprints = BTreeMap::<(PathBuf, PathBuf), BTreeSet<String>>::new();
        let mut best = BTreeMap::<String, ClaudeMcpSnapshot>::new();
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            if name.starts_with('.') || !name.ends_with(".json") {
                continue;
            }
            let Ok(path) = entry.path().canonicalize() else {
                continue;
            };
            if !path.starts_with(&root) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(snapshot) = serde_json::from_slice::<ClaudeMcpSnapshot>(&bytes) else {
                continue;
            };
            if snapshot.version != VERSION || snapshot.servers.iter().any(pending) {
                continue;
            }
            if name != format!("{}.json", snapshot.key.digest()) {
                continue;
            }
            let fresh = fingerprints
                .entry((
                    snapshot.key.config_root.clone(),
                    snapshot.key.project_dir.clone(),
                ))
                .or_insert_with(|| {
                    candidate_fingerprints(&snapshot.key.config_root, &snapshot.key.project_dir)
                });
            if !fresh.contains(&snapshot.key.fingerprint) {
                continue;
            }
            let digest = snapshot.key.digest();
            let replace = match best.get(&snapshot.key.fingerprint) {
                Some(prev) => {
                    snapshot.captured_at_ms > prev.captured_at_ms
                        || (snapshot.captured_at_ms == prev.captured_at_ms
                            && digest > prev.key.digest())
                }
                None => true,
            };
            if replace {
                best.insert(snapshot.key.fingerprint.clone(), snapshot);
            }
        }
        best.into_values().collect()
    }
}

fn atomic_private_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("missing capture directory"))?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent)?;
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
        serde_json::to_writer(&mut file, value)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    let _ = std::fs::remove_file(tmp);
    result
}

#[derive(Clone, Debug)]
pub struct FingerprintInputs {
    pub config_root: PathBuf,
    pub config_file: PathBuf,
    pub project_dir: PathBuf,
    pub managed_dirs: Vec<PathBuf>,
}

impl FingerprintInputs {
    pub fn for_probe(project: Option<&Path>) -> io::Result<Self> {
        Self::for_run(project, &BTreeMap::new())
    }

    pub fn for_run(project: Option<&Path>, env: &BTreeMap<String, String>) -> io::Result<Self> {
        let config_root = super::config_root(env);
        let config_file = if env.get("CLAUDE_CONFIG_DIR").is_some_and(|v| !v.is_empty())
            || (!env.contains_key("HOME")
                && std::env::var_os("CLAUDE_CONFIG_DIR").is_some_and(|v| !v.is_empty()))
        {
            config_root.join(".claude.json")
        } else {
            env.get("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(crate::executable::home_or_current_dir)
                .join(".claude.json")
        };
        let managed = platform_managed_dir();
        Ok(Self {
            config_root: absolute(&config_root)?,
            config_file,
            project_dir: absolute(
                &project
                    .map(Path::to_path_buf)
                    .unwrap_or(std::env::current_dir()?),
            )?,
            managed_dirs: vec![managed],
        })
    }

    pub fn key(&self, cli: &Path) -> io::Result<CaptureKey> {
        Ok(CaptureKey {
            config_root: absolute(&self.config_root)?,
            project_dir: absolute(&self.project_dir)?,
            fingerprint: fingerprint(self)?,
            cli_binary_identity: binary_identity(cli)?,
        })
    }
}

/// Layout used when `config_root` itself holds `.claude.json`.
/// The home layout (`~/.claude.json` with config root `~/.claude`) is the
/// other candidate [`SnapshotStore::latest_fresh`] tries, and only when
/// the config root's file name is `.claude`.
pub fn fresh_capture_key(
    config_root: &Path,
    project_dir: &Path,
    cli_identity: &str,
) -> io::Result<CaptureKey> {
    let config_root = absolute(config_root)?;
    let inputs = FingerprintInputs {
        config_file: config_root.join(".claude.json"),
        config_root: config_root.clone(),
        project_dir: absolute(project_dir)?,
        managed_dirs: vec![platform_managed_dir()],
    };
    Ok(CaptureKey {
        config_root,
        project_dir: inputs.project_dir.clone(),
        fingerprint: fingerprint(&inputs)?,
        cli_binary_identity: cli_identity.to_owned(),
    })
}

/// Claude Code's managed-settings directory on this platform.
pub fn platform_managed_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ClaudeCode")
    } else if cfg!(windows) {
        PathBuf::from(r"C:\Program Files\ClaudeCode")
    } else {
        PathBuf::from("/etc/claude-code")
    }
}

fn candidate_fingerprints(config_root: &Path, project_dir: &Path) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut consider = |config_file: PathBuf| {
        let inputs = FingerprintInputs {
            config_file,
            config_root: config_root.to_path_buf(),
            project_dir: project_dir.to_path_buf(),
            managed_dirs: vec![platform_managed_dir()],
        };
        if let Ok(fingerprint) = fingerprint(&inputs) {
            found.insert(fingerprint);
        }
    };
    consider(config_root.join(".claude.json"));
    if config_root.file_name().and_then(|name| name.to_str()) == Some(".claude")
        && let Some(home) = config_root.parent()
    {
        consider(home.join(".claude.json"));
    }
    found
}

/// Same identity as engine::sessions::binary_identity; does not run the CLI.
pub fn binary_identity(cli: &Path) -> io::Result<String> {
    let path = cli.canonicalize()?;
    let metadata = path.metadata()?;
    Ok(format!(
        "{}:{:?}:{}",
        path.display(),
        metadata.modified().ok(),
        metadata.len()
    ))
}

/// Canonical JSON objects and sorted path sets make formatting/key order irrelevant.
/// Read/parse errors fail closed instead of fingerprinting an apparently empty fleet.
pub fn fingerprint(inputs: &FingerprintInputs) -> io::Result<String> {
    let mut files = BTreeMap::<PathBuf, Value>::new();
    let global = read_json(&inputs.config_file)?;
    let project_root = inputs
        .project_dir
        .ancestors()
        .find(|p| p.join(".git").exists())
        .unwrap_or(&inputs.project_dir);
    let common_root = common_project_root(project_root)?;
    let mut project_config = BTreeMap::new();
    // V-10: Claude keys local MCP by the PRIMARY checkout, even in worktrees.
    // Include cwd/worktree too for conservative invalidation on other releases.
    for dir in [&inputs.project_dir, project_root, &common_root] {
        let key = dir.to_string_lossy();
        project_config.insert(
            key.to_string(),
            select(
                global
                    .get("projects")
                    .and_then(|p| p.get(key.as_ref()))
                    .unwrap_or(&Value::Null),
                MCP_KEYS,
            ),
        );
    }
    files.insert(
        inputs.config_file.clone(),
        json!({
            "global": select(&global, &["mcpServers"]), "projects": project_config,
        }),
    );
    files.insert(
        inputs.project_dir.join(".mcp.json"),
        read_json(&inputs.project_dir.join(".mcp.json"))?,
    );
    if project_root != inputs.project_dir {
        files.insert(
            project_root.join(".mcp.json"),
            read_json(&project_root.join(".mcp.json"))?,
        );
    }
    let mut settings = vec![inputs.config_root.join("settings.json")];
    // Claude reads ancestor settings too. Stop at its repository boundary.
    for dir in inputs.project_dir.ancestors() {
        settings.push(dir.join(".claude/settings.json"));
        settings.push(dir.join(".claude/settings.local.json"));
        if dir == project_root && dir.join(".git").exists() {
            break;
        }
    }
    for dir in &inputs.managed_dirs {
        settings.push(dir.join("managed-settings.json"));
        settings.extend(sorted_children(&dir.join("managed-settings.d"))?);
        files.insert(
            dir.join("managed-mcp.json"),
            json!(dir.join("managed-mcp.json").try_exists()?),
        );
    }
    for path in settings {
        files.insert(path.clone(), select(&read_json(&path)?, SETTINGS_KEYS));
    }
    let installed_path = inputs.config_root.join("plugins/installed_plugins.json");
    let installed = read_json(&installed_path)?;
    files.insert(installed_path, installed.clone());
    let marketplaces = inputs.config_root.join("plugins/known_marketplaces.json");
    files.insert(marketplaces.clone(), read_json(&marketplaces)?);
    for root in sorted_children(&inputs.config_root.join("plugins/synced"))? {
        files.insert(
            root.join("manifest.json"),
            metadata_stamp(&root.join("manifest.json"))?,
        );
    }
    if let Some(plugins) = installed.get("plugins").and_then(Value::as_object) {
        for entries in plugins.values().filter_map(Value::as_array) {
            for path in entries
                .iter()
                .filter_map(|p| p.get("installPath"))
                .filter_map(Value::as_str)
            {
                for suffix in [".mcp.json", ".claude-plugin/plugin.json"] {
                    let path = Path::new(path).join(suffix);
                    files.insert(path.clone(), metadata_stamp(&path)?);
                }
            }
        }
    }
    Ok(digest(
        &serde_json::to_value(files).map_err(io::Error::other)?,
    ))
}

fn read_json(path: &Path) -> io::Result<Value> {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(value @ Value::Object(_)) => Ok(value),
            _ => Err(invalid("invalid MCP fingerprint JSON object")),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Value::Null),
        Err(e) => Err(e),
    }
}
fn common_project_root(project: &Path) -> io::Result<PathBuf> {
    let marker = project.join(".git");
    if !marker.is_file() {
        return Ok(project.to_path_buf());
    }
    let contents = std::fs::read_to_string(&marker)?;
    let git_dir = contents
        .trim()
        .strip_prefix("gitdir: ")
        .ok_or_else(|| invalid("invalid git worktree marker"))?;
    let git_dir = project.join(git_dir).canonicalize()?;
    let common = git_dir.join("commondir");
    match std::fs::read_to_string(common) {
        Ok(common) => git_dir
            .join(common.trim())
            .canonicalize()?
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| invalid("git common dir has no parent")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(project.to_path_buf()),
        Err(e) => Err(e),
    }
}
fn sorted_children(path: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = match std::fs::read_dir(path) {
        Ok(entries) => entries
            .map(|entry| entry.map(|e| e.path()))
            .collect::<io::Result<Vec<_>>>()?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    paths.sort();
    Ok(paths)
}
fn metadata_stamp(path: &Path) -> io::Result<Value> {
    match path.metadata() {
        Ok(m) => Ok(
            json!({"size": m.len(), "modified": m.modified()?.duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?.as_nanos().to_string()}),
        ),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Value::Null),
        Err(e) => Err(e),
    }
}
fn select(value: &Value, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .filter_map(|key| value.get(*key).map(|v| ((*key).into(), v.clone())))
            .collect(),
    )
}
fn digest(value: &Value) -> String {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let ordered: BTreeMap<_, _> =
                    map.iter().map(|(k, v)| (k.clone(), canonical(v))).collect();
                serde_json::to_value(ordered).expect("JSON object")
            }
            Value::Array(array) => Value::Array(array.iter().map(canonical).collect()),
            _ => value.clone(),
        }
    }
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&canonical(value)).expect("JSON"))
    )
}
fn absolute(path: &Path) -> io::Result<PathBuf> {
    path.canonicalize()
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn pending(row: &ClaudeMcpServer) -> bool {
    matches!(row.status.as_str(), "pending" | "connecting")
}

pub(super) fn store_for_probe() -> Option<SnapshotStore> {
    let data_dir = std::env::var_os("ZERON_DATA_DIR")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            #[cfg(windows)]
            {
                std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Zeron"))
            }
            #[cfg(not(windows))]
            {
                crate::executable::home_dir().map(|p| p.join(".zeron"))
            }
        })?;
    Some(SnapshotStore::new(&data_dir))
}

/// Continue on the same eager discovery process after initialize. No user frame
/// is ever sent. A timeout/unsupported request invalidates this exact key only.
pub(super) async fn capture(
    stdin: &mut crate::process::ChildStdin,
    lines: &mut crate::lossy_lines::LossyLines<crate::process::ChildStdout>,
) -> io::Result<Vec<ClaudeMcpServer>> {
    tokio::time::timeout(STATUS_TIMEOUT, async {
        loop {
            let request = json!({"type":"control_request", "request_id":STATUS_ID,
                "request":{"subtype":"mcp_status"}});
            stdin.write_all(format!("{request}\n").as_bytes()).await?;
            stdin.flush().await?;
            loop {
                let line = lines
                    .next_line()
                    .await?
                    .ok_or_else(|| invalid("MCP status stdout closed"))?;
                let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if frame
                    .pointer("/response/request_id")
                    .and_then(Value::as_str)
                    != Some(STATUS_ID)
                {
                    continue;
                }
                let rows = parse_status(&frame)?;
                if !rows.iter().any(pending) {
                    return Ok(rows);
                }
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "MCP status timed out"))?
}

/// Once per CLI binary; a negative cached result is an explicit legacy fallback.
/// The supplied command must be zeron mcp-proxy --selftest (or its small sibling).
/// This probe is deliberately independent of command/model catalog discovery.
pub async fn probe_caps(
    cli: &Path,
    proxy: &Path,
    proxy_args: &[String],
    baseline: &[ClaudeMcpServer],
    store: &SnapshotStore,
) -> io::Result<ClaudeMcpCaps> {
    static CAPS_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = CAPS_LOCK.lock().await;
    let identity = binary_identity(cli)?;
    if let Some(caps) = store.load_caps(&identity) {
        return Ok(caps);
    }
    let mut proxy_args = proxy_args.to_vec();
    proxy_args.extend([
        "--data-dir".into(),
        store
            .root
            .parent()
            .ok_or_else(|| invalid("missing probe data dir"))?
            .to_string_lossy()
            .into_owned(),
    ]);
    let mut cmd = Command::new(cli);
    crate::process::owned::configure(&mut cmd);
    crate::compose_child_path(&mut cmd, cli);
    cmd.args([
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--settings",
        r#"{"disableAllHooks":true}"#,
        "--strict-mcp-config",
        "--mcp-config",
    ])
    .arg(
        json!({"mcpServers":{"zeron_caps":{"command":proxy,"args":proxy_args,
            "env":{"ZERON_MCP_CAPS_SENTINEL":"zeron-caps"}}}})
        .to_string(),
    )
    .env("CLAUDE_CODE_AUTO_CONNECT_IDE", "0")
    .env("CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL", "1")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .kill_on_drop(true);
    let mut child = cmd.spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| invalid("caps stdin missing"))?;
    let mut lines = crate::lossy_lines::lossy_lines(
        child
            .stdout
            .take()
            .ok_or_else(|| invalid("caps stdout missing"))?,
    );
    let mut child = crate::process::owned::Child::new(child);
    let result = tokio::time::timeout(STATUS_TIMEOUT, async {
        stdin.write_all(b"{\"type\":\"control_request\",\"request_id\":\"zeron-caps-init\",\"request\":{\"subtype\":\"initialize\"}}\n").await?;
        stdin.flush().await?;
        while let Some(line) = lines.next_line().await? {
            let Ok(frame) = serde_json::from_str::<Value>(&line) else { continue };
            if frame.pointer("/response/request_id").and_then(Value::as_str) == Some("zeron-caps-init") {
                if frame.pointer("/response/subtype").and_then(Value::as_str) != Some("success") {
                    return Err(invalid("caps initialize failed"));
                }
                return capture(&mut stdin, &mut lines).await;
            }
        }
        Err(invalid("caps exited before initialize"))
    }).await;
    super::refresh_lock::settle(
        &super::refresh_lock::config_dir(),
        super::refresh_lock::SETTLE_MAX,
    )
    .await;
    drop(stdin);
    child.shutdown(Duration::from_millis(200)).await;
    let caps = match result {
        Ok(Ok(rows)) => ClaudeMcpCaps::from_rows(identity, baseline, &rows),
        _ => ClaudeMcpCaps {
            version: VERSION,
            cli_binary_identity: identity,
            strict_drops_plugins: false,
            strict_keeps_claudeai: None,
            status_has_config: false,
            status_preserves_env: false,
            probed_at_ms: now_ms(),
        },
    };
    store.save_caps(&caps)?;
    Ok(caps)
}

pub(super) fn selftest_command() -> Option<(PathBuf, Vec<String>)> {
    let exe = std::env::current_exe().ok()?;
    let sibling = exe.parent()?.join(if cfg!(windows) {
        "zeron-mcp-proxy.exe"
    } else {
        "zeron-mcp-proxy"
    });
    if sibling.is_file() {
        return Some((sibling, vec!["--selftest".into()]));
    }
    let app = if exe.file_stem()?.to_str()? == "zeron" {
        exe
    } else {
        exe.parent()?
            .join(if cfg!(windows) { "zeron.exe" } else { "zeron" })
    };
    app.is_file()
        .then(|| (app, vec!["mcp-proxy".into(), "--selftest".into()]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let fixtures: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/claude_mcp_status.json"))
                .unwrap();
        fixtures[name].clone()
    }
    fn inputs() -> (tempfile::TempDir, FingerprintInputs) {
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("config");
        let project_dir = dir.path().join("project");
        let managed = dir.path().join("managed");
        for path in [&config_root, &project_dir, &managed] {
            std::fs::create_dir_all(path).unwrap();
        }
        std::fs::create_dir(project_dir.join(".git")).unwrap();
        let inputs = FingerprintInputs {
            config_file: config_root.join(".claude.json"),
            config_root,
            project_dir,
            managed_dirs: vec![managed],
        };
        (dir, inputs)
    }
    fn write(path: &Path, value: Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }
    fn key() -> CaptureKey {
        CaptureKey {
            config_root: "/config".into(),
            project_dir: "/project".into(),
            fingerprint: "fingerprint".into(),
            cli_binary_identity: "cli-id".into(),
        }
    }

    #[test]
    fn recorded_status_keeps_all_sources_and_tools_without_inventing_env() {
        let rows = parse_status(&fixture("baseline")).unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].name, "plugin:playwright:playwright");
        assert_eq!(rows[0].source.as_deref(), Some("plugin"));
        assert_eq!(rows[0].scope.as_deref(), Some("dynamic"));
        assert_eq!(rows[0].tools.as_ref().unwrap()[0]["name"], "browser_click");
        assert_eq!(rows[0].extra["serverInfo"]["version"], "1.0.0");
        assert!(rows.iter().all(|r| !r.stdio_config_complete()));
        assert_eq!(rows[3].scope.as_deref(), Some("local"));
        assert_eq!(
            parse_status(&fixture("override")).unwrap()[0]
                .source
                .as_deref(),
            Some("dynamic")
        );
        assert_eq!(
            parse_status(&fixture("normalized")).unwrap()[0].name,
            "plugin_playwright_playwright"
        );
        let connector = parse_status(&fixture("connector-redeclare")).unwrap();
        assert_eq!(connector[0].status, "failed");
        assert_eq!(connector[0].transport.as_deref(), Some("claudeai-proxy"));
        assert_eq!(
            connector[0].extra["error"],
            "No claude.ai OAuth token found"
        );
    }
    #[test]
    fn status_errors_and_malformed_rows_do_not_become_an_empty_snapshot() {
        for frame in [
            Value::Null,
            json!({"type":"control_response","response":{"subtype":"error"}}),
            json!({"type":"control_response","response":{"subtype":"success","response":{}}}),
        ] {
            assert!(parse_status(&frame).is_err());
        }
        for rows in [
            json!([{"name":"x"}]),
            json!([{"name":"","status":"connected"}]),
            json!([{"name":"x","status":"connected"},{"name":"x","status":"failed"}]),
        ] {
            assert!(
                parse_status(
                    &json!({"type":"control_response","response":{"subtype":"success",
                "response":{"mcpServers":rows}}})
                )
                .is_err()
            );
        }
    }
    #[test]
    fn unknown_transports_and_missing_config_are_preserved_for_legacy_fallback() {
        let mut frame = fixture("baseline");
        let row = &mut frame["response"]["response"]["mcpServers"][0];
        row.as_object_mut().unwrap().remove("config");
        row["transport"] = json!("future");
        row["channelCapabilities"] = json!({"claude/channel":{}});
        let rows = parse_status(&frame).unwrap();
        assert!(rows[0].config.is_none());
        assert_eq!(
            rows[0].extra["channelCapabilities"],
            json!({"claude/channel":{}})
        );
        assert!(!rows[0].stdio_config_complete());
    }
    #[test]
    fn caps_never_claims_connectors_survive_when_none_were_observed() {
        let baseline = parse_status(&fixture("baseline")).unwrap();
        let rows = parse_status(&fixture("strict")).unwrap();
        let caps = ClaudeMcpCaps::from_rows("id".into(), &baseline, &rows);
        assert!(caps.strict_drops_plugins && caps.status_has_config);
        assert_eq!(caps.strict_keeps_claudeai, None);
        assert!(!caps.status_preserves_env);
        assert!(!caps.permits_rewrite());
        let caps = ClaudeMcpCaps::from_rows("id".into(), &baseline, &baseline);
        assert!(!caps.strict_drops_plugins);
        assert!(!caps.status_has_config);
    }
    #[test]
    fn caps_requires_real_selftest_tool_and_explicit_env_roundtrip() {
        let mut rows = parse_status(&fixture("strict")).unwrap();
        rows[0].config.as_mut().unwrap()["env"] = json!({"ZERON_MCP_CAPS_SENTINEL":"zeron-caps"});
        assert!(ClaudeMcpCaps::from_rows("id".into(), &[], &rows).permits_rewrite());
        rows[0].source = Some("plugin".into());
        assert!(!ClaudeMcpCaps::from_rows("id".into(), &[], &rows).strict_drops_plugins);
        rows[0].source = Some("dynamic".into());
        rows[0].tools = Some(vec![]);
        assert!(!ClaudeMcpCaps::from_rows("id".into(), &[], &rows).permits_rewrite());
    }
    #[test]
    fn fingerprint_ignores_format_order_usage_and_unrelated_projects() {
        let (_dir, inputs) = inputs();
        let path = &inputs.config_file;
        write(
            path,
            json!({"mcpServers":{"x":{"args":["a"],"command":"cmd"}},"lastUsed":1}),
        );
        let first = fingerprint(&inputs).unwrap();
        std::fs::write(
            path,
            r#"{ "lastUsed": 2, "projects": {"/other":{"mcpServers":{"secret":{}}}},
            "mcpServers": {"x":{"command":"cmd","args":["a"]}} }"#,
        )
        .unwrap();
        assert_eq!(fingerprint(&inputs).unwrap(), first);
        write(
            path,
            json!({"mcpServers":{"x":{"args":["b"],"command":"cmd"}}}),
        );
        assert_ne!(fingerprint(&inputs).unwrap(), first);
    }
    #[test]
    fn fingerprint_includes_every_project_mcp_toggle() {
        let (_dir, inputs) = inputs();
        let first = fingerprint(&inputs).unwrap();
        for toggle in MCP_KEYS {
            write(
                &inputs.config_file,
                json!({"projects":{inputs.project_dir.to_string_lossy().as_ref():{*toggle:true}}}),
            );
            assert_ne!(fingerprint(&inputs).unwrap(), first, "{toggle}");
        }
    }
    #[test]
    fn fingerprint_covers_settings_layers_and_managed_command_policies() {
        let (_dir, inputs) = inputs();
        let first = fingerprint(&inputs).unwrap();
        for path in [
            inputs.config_root.join("settings.json"),
            inputs.project_dir.join(".claude/settings.json"),
            inputs.project_dir.join(".claude/settings.local.json"),
            inputs.managed_dirs[0].join("managed-settings.json"),
            inputs.managed_dirs[0].join("managed-settings.d/one.json"),
        ] {
            for setting in SETTINGS_KEYS {
                write(&path, json!({*setting:[{"serverCommand":["restricted"]}]}));
                assert_ne!(fingerprint(&inputs).unwrap(), first, "{path:?} {setting}");
            }
            std::fs::remove_file(path).unwrap();
        }
        write(&inputs.managed_dirs[0].join("managed-mcp.json"), json!({}));
        assert_ne!(fingerprint(&inputs).unwrap(), first);
    }
    #[test]
    fn fingerprint_plugin_state_and_metadata_change_without_reading_credentials() {
        let (dir, inputs) = inputs();
        let first = fingerprint(&inputs).unwrap();
        std::fs::write(
            inputs.config_root.join(".credentials.json"),
            "DO NOT READ invalid JSON",
        )
        .unwrap();
        assert_eq!(fingerprint(&inputs).unwrap(), first);
        let root = dir.path().join("plugin");
        write(
            &inputs.config_root.join("plugins/installed_plugins.json"),
            json!({"plugins":{"p@m":[{"installPath":root}]}}),
        );
        let installed = fingerprint(&inputs).unwrap();
        assert_ne!(installed, first);
        for path in [
            root.join(".mcp.json"),
            root.join(".claude-plugin/plugin.json"),
            inputs.config_root.join("plugins/synced/id/manifest.json"),
        ] {
            let before = fingerprint(&inputs).unwrap();
            write(&path, json!({"new":true}));
            assert_ne!(fingerprint(&inputs).unwrap(), before);
        }
        let before = fingerprint(&inputs).unwrap();
        write(
            &inputs.config_root.join("plugins/known_marketplaces.json"),
            json!({"p":{}}),
        );
        assert_ne!(fingerprint(&inputs).unwrap(), before);
    }
    #[test]
    fn fingerprint_worktree_tracks_primary_checkout_project_key() {
        let (dir, mut inputs) = inputs();
        let main = dir.path().join("main");
        let git_dir = main.join(".git/worktrees/linked");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(git_dir.join("commondir"), "../..\n").unwrap();
        let linked = inputs.project_dir.clone();
        std::fs::remove_dir(linked.join(".git")).unwrap();
        std::fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .unwrap();
        let nested = linked.join("nested");
        std::fs::create_dir(&nested).unwrap();
        inputs.project_dir = nested;
        assert_eq!(common_project_root(&linked).unwrap(), main);
        let before = fingerprint(&inputs).unwrap();
        write(
            &inputs.config_file,
            json!({"projects":{main.to_string_lossy().as_ref():{"disabledMcpServers":["x"]}}}),
        );
        assert_ne!(fingerprint(&inputs).unwrap(), before);
        for name in ["worktree-key", "nested-key"] {
            let rows = parse_status(&fixture(name)).unwrap();
            assert!(
                rows.iter()
                    .any(|r| r.name == "main_key" && r.scope.as_deref() == Some("local"))
            );
            assert!(
                !rows
                    .iter()
                    .any(|r| matches!(r.name.as_str(), "worktree_key" | "nested_key"))
            );
        }
    }
    #[test]
    fn fingerprint_rejects_bad_config_instead_of_reusing_a_capture() {
        let (_dir, inputs) = inputs();
        std::fs::write(&inputs.config_file, "bad config secret").unwrap();
        let error = fingerprint(&inputs).unwrap_err().to_string();
        assert!(!error.contains("secret"));
        assert!(inputs.key(Path::new("not-a-cli")).is_err());
    }
    #[test]
    fn snapshot_roundtrip_is_private_and_atomic_and_every_key_field_invalidates() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        let mut snapshot =
            ClaudeMcpSnapshot::new(key(), parse_status(&fixture("baseline")).unwrap());
        store.save(&snapshot).unwrap();
        assert_eq!(store.load(&snapshot.key), Some(snapshot.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                store
                    .path(&snapshot.key)
                    .metadata()
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                store.root.metadata().unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        for field in [
            "config_root",
            "project_dir",
            "fingerprint",
            "cli_binary_identity",
        ] {
            let mut value = serde_json::to_value(&snapshot.key).unwrap();
            value[field] = json!("different");
            let other = serde_json::from_value(value).unwrap();
            assert_ne!(store.path(&snapshot.key), store.path(&other));
            assert!(store.load(&other).is_none());
        }
        snapshot.servers.clear();
        store.save(&snapshot).unwrap();
        assert_eq!(store.load(&snapshot.key).unwrap().servers.len(), 0);
        assert_eq!(std::fs::read_dir(&store.root).unwrap().count(), 1);
        store.invalidate(&snapshot.key).unwrap();
        store.invalidate(&snapshot.key).unwrap();
        assert!(store.load(&snapshot.key).is_none());
    }
    #[test]
    fn corrupt_wrong_version_wrong_key_and_pending_snapshots_are_misses() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        let mut snapshot =
            ClaudeMcpSnapshot::new(key(), parse_status(&fixture("baseline")).unwrap());
        store.save(&snapshot).unwrap();
        std::fs::write(store.path(&snapshot.key), "corrupt").unwrap();
        assert!(store.load(&snapshot.key).is_none());
        snapshot.version = VERSION + 1;
        write(
            &store.path(&snapshot.key),
            serde_json::to_value(&snapshot).unwrap(),
        );
        assert!(store.load(&snapshot.key).is_none());
        assert!(store.save(&snapshot).is_err());
        snapshot.version = VERSION;
        snapshot.servers[0].status = "pending".into();
        assert!(store.save(&snapshot).is_err());
        snapshot.servers[0].status = "connected".into();
        let requested_key = snapshot.key.clone();
        snapshot.key.fingerprint = "wrong".into();
        write(
            &store.path(&requested_key),
            serde_json::to_value(&snapshot).unwrap(),
        );
        assert!(store.load(&requested_key).is_none());
    }
    #[test]
    fn caps_cache_is_versioned_and_keyed_by_cli_identity_including_negative_results() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        let caps = ClaudeMcpCaps::from_rows("old".into(), &[], &[]);
        store.save_caps(&caps).unwrap();
        assert_eq!(store.load_caps("old"), Some(caps));
        assert!(store.load_caps("new").is_none());
        write(&store.root.join("caps.json"), json!({"version":99}));
        assert!(store.load_caps("old").is_none());
    }
    #[test]
    fn cli_identity_changes_when_replaced_and_does_not_execute_it() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("claude");
        std::fs::write(&exe, "not executable").unwrap();
        let before = binary_identity(&exe).unwrap();
        std::fs::write(&exe, "a replacement executable").unwrap();
        assert_ne!(binary_identity(&exe).unwrap(), before);
        assert_eq!(
            binary_identity(&exe).unwrap(),
            binary_identity(&exe).unwrap()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn caps_probe_uses_only_controls_and_caches_the_negative_result() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let cli = dir.path().join("claude");
        std::fs::write(&cli, r#"#!/usr/bin/python3
import json, pathlib, sys
root = pathlib.Path(__file__).parent
for line in sys.stdin:
    request = json.loads(line)
    assert request['type'] == 'control_request', 'model message forbidden'
    subtype = request['request']['subtype']
    with (root / 'controls').open('a') as f: f.write(subtype + '\n')
    if subtype == 'initialize':
        response = {}
    elif subtype == 'mcp_status':
        config = json.loads(sys.argv[sys.argv.index('--mcp-config') + 1])['mcpServers']['zeron_caps']
        assert config['env']['ZERON_MCP_CAPS_SENTINEL'] == 'zeron-caps'
        assert config['args'][-2:] == ['--data-dir', str(root)]
        assert '--strict-mcp-config' in sys.argv
        # Mirror the installed CLI's omission, to exercise the safe fallback.
        del config['env']
        response = {'mcpServers':[{'name':'zeron_caps','status':'connected',
            'config':config,'source':'dynamic','scope':'dynamic','transport':'stdio',
            'tools':[{'name':'zeron_proxy_selftest'}]}]}
    else: raise AssertionError(subtype)
    print(json.dumps({'type':'control_response','response':{'subtype':'success',
        'request_id':request['request_id'],'response':response}}), flush=True)
"#).unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        let store = SnapshotStore::new(dir.path());
        let first = probe_caps(
            &cli,
            Path::new("/fixture/zeron"),
            &["mcp-proxy".into(), "--selftest".into()],
            &[],
            &store,
        )
        .await
        .unwrap();
        assert!(first.status_has_config && first.strict_drops_plugins);
        assert!(!first.permits_rewrite());
        let second = probe_caps(&cli, Path::new("/fixture/zeron"), &[], &[], &store)
            .await
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("controls")).unwrap(),
            "initialize\nmcp_status\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn discovery_returns_while_status_capture_is_persisted_in_background() {
        use crate::Harness;
        use std::os::unix::fs::PermissionsExt;
        // Its own environment: never create a snapshot under a developer's
        // actual data/config root, and do not mutate other tests' environment.
        if std::env::var_os("ZERON_MCP_CAPTURE_TEST_CHILD").is_none() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir(dir.path().join("config")).unwrap();
            std::fs::create_dir(dir.path().join(".git")).unwrap();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "claude::mcp_capture::tests::discovery_returns_while_status_capture_is_persisted_in_background"])
                .env("ZERON_MCP_CAPTURE_TEST_CHILD", "1")
                .env("CLAUDE_CONFIG_DIR", dir.path().join("config"))
                .env("ZERON_DATA_DIR", dir.path().join("data"))
                .current_dir(dir.path())
                .status().unwrap();
            assert!(status.success());
            return;
        }
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let cli = dir.path().join("claude");
            std::fs::write(
                &cli,
                r#"#!/usr/bin/python3
import json, pathlib, sys
root = pathlib.Path(__file__).parent
for line in sys.stdin:
    req = json.loads(line)
    assert req['type'] == 'control_request', 'model message forbidden'
    if req['request']['subtype'] == 'initialize':
        if (root / 'fail').exists():
            print(json.dumps({'type':'control_response','response':{'subtype':'error',
                'request_id':req['request_id'],'error':'fixture initialize failure'}}), flush=True)
            continue
        response = {'commands':[{'name':'capture-test','description':'fixture'}]}
    elif req['request']['subtype'] == 'mcp_status':
        response = {'mcpServers':[]}
    else: raise AssertionError(req)
    print(json.dumps({'type':'control_response','response':{'subtype':'success',
        'request_id':req['request_id'],'response':response}}), flush=True)
"#,
            )
            .unwrap();
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
            let key = FingerprintInputs::for_probe(None)
                .unwrap()
                .key(&cli)
                .unwrap();
            let store = store_for_probe().unwrap();
            let harness = crate::ClaudeHarness::new().with_executable(&cli);
            assert_eq!(harness.commands().await.unwrap()[0].name, "capture-test");
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Some(snapshot) = store.load(&key) {
                        assert!(snapshot.servers.is_empty());
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
            // A failed probe with the same CLI/config key invalidates the prior
            // successful capture, while still returning the initialize error.
            std::fs::write(dir.path().join("fail"), "").unwrap();
            let failing = crate::ClaudeHarness::new().with_executable(&cli);
            assert!(failing.commands().await.is_err());
            assert!(store.load(&key).is_none());
        });
    }
}
