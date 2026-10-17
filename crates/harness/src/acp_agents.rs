//! User-added ACP agents: any agent listed in the official ACP registry
//! (<https://agentclientprotocol.com/registry>) or any local command that
//! speaks ACP over stdio. Each one runs through the shared [`AcpHarness`]
//! (see [`AcpHarness::custom`]) under its own [`HarnessId::Acp`] id.
//!
//! Registry agents are pinned to the release that was current when they
//! were added and installed up front ([`AcpAgentConfig::prepare`]), so a
//! chat turn never waits on a download:
//! - `binary` — the platform's archive, checked against the registry's
//!   SHA-256 and unpacked under the managed adapters dir;
//! - `npx` — the package installed once into the managed npm prefix, then
//!   spawned with `node` (never `npx` in the hot path, see
//!   [`crate::adapter_install`]);
//! - `uvx` — spawned through `uvx`, which keeps its own tool cache.
//!
//! [`AcpHarness`]: crate::AcpHarness
//! [`AcpHarness::custom`]: crate::AcpHarness::custom

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use zeron_proto::{HarnessId, Model};

use crate::HarnessError;
use crate::adapter_install::NpmPin;
use crate::archive_install::RegistryRelease;

/// The registry index served by the ACP project's CDN.
pub const REGISTRY_URL: &str =
    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";
/// Prefix of every user-added agent's harness id (`acp.gemini`).
pub const ID_PREFIX: &str = "acp.";

const REGISTRY_TTL: Duration = Duration::from_secs(10 * 60);
const REGISTRY_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REGISTRY_BYTES: usize = 4 * 1024 * 1024;
const REGISTRY_CACHE_FILE: &str = "acp-registry.json";

/// One registry entry — the fields Zeron uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistryAgent {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default)]
    pub distribution: RegistryDistribution,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RegistryDistribution {
    /// Keyed by platform (`darwin-aarch64`, `linux-x86_64`, `windows-x86_64`, …).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub binary: BTreeMap<String, BinaryTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npx: Option<PackageDistribution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uvx: Option<PackageDistribution>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BinaryTarget {
    pub archive: String,
    /// The executable, relative to the archive root (`./goose`).
    pub cmd: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackageDistribution {
    /// Pinned package (`@google/gemini-cli@0.63.0`, `fast-agent-acp==0.10.1`).
    pub package: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl RegistryAgent {
    /// The harness id this agent runs under once added.
    pub fn harness_id(&self) -> Option<HarnessId> {
        HarnessId::parse(&format!("{ID_PREFIX}{}", self.id)).filter(HarnessId::is_acp)
    }

    /// How this device would run the agent: `Err` says what's missing.
    pub fn launch_for_this_device(&self) -> Result<AcpLaunch, String> {
        choose_distribution(self, &Tools::detect())
    }

    /// The icon url: the registry's own, else the CDN's per-agent path.
    pub fn icon_url(&self) -> String {
        self.icon.clone().unwrap_or_else(|| {
            format!(
                "https://cdn.agentclientprotocol.com/registry/v1/latest/{}.svg",
                self.id
            )
        })
    }
}

/// Which runners this device has, for picking a distribution.
struct Tools {
    platform: Option<&'static str>,
    npm: bool,
    uvx: bool,
}

impl Tools {
    fn detect() -> Self {
        Self {
            platform: platform_key(),
            npm: crate::adapter_install::find_npm().is_some(),
            uvx: find_uvx().is_some(),
        }
    }
}

/// The registry's platform key for this build.
pub fn platform_key() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-aarch64",
        ("macos", "x86_64") => "darwin-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        ("linux", "x86_64") => "linux-x86_64",
        ("windows", "aarch64") => "windows-aarch64",
        ("windows", "x86_64") => "windows-x86_64",
        _ => return None,
    })
}

fn find_uvx() -> Option<PathBuf> {
    crate::executable::find_on_paths("uvx", Vec::new())
}

/// A prebuilt binary first (no runtime needed), then npm, then uv — the
/// registry's own preference order.
fn choose_distribution(agent: &RegistryAgent, tools: &Tools) -> Result<AcpLaunch, String> {
    let dist = &agent.distribution;
    if let Some(target) = tools.platform.and_then(|key| dist.binary.get(key)) {
        return Ok(AcpLaunch::Binary {
            archive: target.archive.clone(),
            cmd: target.cmd.clone(),
            args: target.args.clone(),
            env: target.env.clone(),
            sha256: target.sha256.clone(),
        });
    }
    if let Some(npx) = dist.npx.as_ref().filter(|_| tools.npm) {
        return Ok(AcpLaunch::Npx {
            package: npx.package.clone(),
            args: npx.args.clone(),
            env: npx.env.clone(),
        });
    }
    if let Some(uvx) = dist.uvx.as_ref().filter(|_| tools.uvx) {
        return Ok(AcpLaunch::Uvx {
            package: uvx.package.clone(),
            args: uvx.args.clone(),
            env: uvx.env.clone(),
        });
    }
    Err(if dist.npx.is_some() {
        "Needs Node.js (npm) on this device".into()
    } else if dist.uvx.is_some() {
        "Needs uv on this device".into()
    } else {
        "No build for this device".into()
    })
}

/// Parse a registry index, dropping (not failing on) entries this build
/// can't use: a malformed agent must not hide the rest.
pub fn parse_registry(bytes: &[u8]) -> Result<Vec<RegistryAgent>, HarnessError> {
    let index: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| HarnessError::Protocol(format!("invalid ACP registry: {error}")))?;
    let agents = index
        .get("agents")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| HarnessError::Protocol("ACP registry has no agents list".into()))?;
    let mut parsed: Vec<RegistryAgent> = agents
        .iter()
        .filter_map(|agent| serde_json::from_value::<RegistryAgent>(agent.clone()).ok())
        .filter(|agent| agent.harness_id().is_some() && !agent.name.trim().is_empty())
        .collect();
    parsed.sort_by_cached_key(|agent| agent.name.to_lowercase());
    Ok(parsed)
}

type RegistryCache = Mutex<Option<(Instant, Arc<Vec<RegistryAgent>>)>>;

fn registry_cache() -> &'static RegistryCache {
    static CACHE: RegistryCache = Mutex::new(None);
    &CACHE
}

/// The registry, fetched at most every ten minutes (`force` refetches). A
/// failed fetch falls back to the last index saved on disk.
pub async fn registry(force: bool) -> Result<Arc<Vec<RegistryAgent>>, HarnessError> {
    if !force
        && let Some((at, agents)) = registry_cache()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        && at.elapsed() < REGISTRY_TTL
    {
        return Ok(agents.clone());
    }
    let disk = crate::adapter_install::adapters_root().map(|root| root.join(REGISTRY_CACHE_FILE));
    let agents = match fetch_registry().await {
        Ok((bytes, agents)) => {
            if let Some(path) = &disk {
                let _ = write_atomically(path, &bytes);
            }
            agents
        }
        Err(error) => {
            let cached = disk
                .as_ref()
                .and_then(|path| std::fs::read(path).ok())
                .and_then(|bytes| parse_registry(&bytes).ok());
            match cached {
                Some(agents) => {
                    tracing::warn!(%error, "ACP registry fetch failed; using the saved index");
                    agents
                }
                None => return Err(error),
            }
        }
    };
    let agents = Arc::new(agents);
    *registry_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some((Instant::now(), agents.clone()));
    Ok(agents)
}

async fn fetch_registry() -> Result<(Vec<u8>, Vec<RegistryAgent>), HarnessError> {
    let failed = |detail: String| {
        HarnessError::Protocol(format!("couldn't load the ACP registry: {detail}"))
    };
    let client = reqwest::Client::builder()
        .timeout(REGISTRY_TIMEOUT)
        .build()
        .map_err(|error| failed(error.to_string()))?;
    let response = client
        .get(REGISTRY_URL)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| failed(error.to_string()))?;
    let bytes = response
        .bytes()
        .await
        .map_err(|error| failed(error.to_string()))?;
    if bytes.len() > MAX_REGISTRY_BYTES {
        return Err(failed("the index is unexpectedly large".into()));
    }
    let agents = parse_registry(&bytes)?;
    Ok((bytes.to_vec(), agents))
}

fn write_atomically(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// One user-added agent, as persisted on its device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpAgentConfig {
    pub id: HarnessId,
    pub name: String,
    /// The registry entry this came from; `None` for a custom command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub launch: AcpLaunch,
}

/// How a user-added agent starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AcpLaunch {
    /// A local executable (name on PATH or a path) and its arguments.
    Command {
        program: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    Npx {
        package: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    Uvx {
        package: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    Binary {
        archive: String,
        cmd: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default)]
        sha256: Option<String>,
    },
}

impl AcpLaunch {
    pub fn env(&self) -> &BTreeMap<String, String> {
        match self {
            Self::Command { env, .. }
            | Self::Npx { env, .. }
            | Self::Uvx { env, .. }
            | Self::Binary { env, .. } => env,
        }
    }

    pub fn env_mut(&mut self) -> &mut BTreeMap<String, String> {
        match self {
            Self::Command { env, .. }
            | Self::Npx { env, .. }
            | Self::Uvx { env, .. }
            | Self::Binary { env, .. } => env,
        }
    }

    pub fn args(&self) -> &[String] {
        match self {
            Self::Command { args, .. }
            | Self::Npx { args, .. }
            | Self::Uvx { args, .. }
            | Self::Binary { args, .. } => args,
        }
    }
}

/// What a resolved launch spawns.
pub(crate) enum Resolved {
    Program(PathBuf, Vec<String>),
    /// A managed npm install (installed on demand when missing).
    Npm {
        pin: NpmPin,
        bin_name: &'static str,
        args: Vec<String>,
    },
}

impl AcpAgentConfig {
    /// A registry agent, pinned to the listed release and the distribution
    /// this device can run.
    pub fn from_registry(agent: &RegistryAgent) -> Result<Self, String> {
        let id = agent
            .harness_id()
            .ok_or_else(|| format!("unsupported agent id `{}`", agent.id))?;
        Ok(Self {
            id,
            name: agent.name.trim().to_owned(),
            registry_id: Some(agent.id.clone()),
            version: Some(agent.version.clone()),
            description: Some(agent.description.clone()).filter(|d| !d.trim().is_empty()),
            icon: Some(agent.icon_url()),
            launch: agent.launch_for_this_device()?,
        })
    }

    /// A local command. `id_suffix` disambiguates repeated names.
    pub fn command(
        name: &str,
        program: &str,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        id_suffix: Option<usize>,
    ) -> Result<Self, String> {
        let name = name.trim();
        let program = program.trim();
        if program.is_empty() {
            return Err("Enter the command that starts the agent".into());
        }
        let name = if name.is_empty() {
            program_label(program)
        } else {
            name.to_owned()
        };
        let mut slug = slug(&name);
        if slug.is_empty() {
            slug = "agent".into();
        }
        let suffix = id_suffix.map(|n| format!("-{n}")).unwrap_or_default();
        let id = HarnessId::parse(&format!("{ID_PREFIX}custom-{slug}{suffix}"))
            .filter(HarnessId::is_acp)
            .ok_or_else(|| "That name can't be used".to_string())?;
        Ok(Self {
            id,
            name,
            registry_id: None,
            version: None,
            description: None,
            icon: None,
            launch: AcpLaunch::Command {
                program: program.to_owned(),
                args,
                env,
            },
        })
    }

    pub fn env(&self) -> &BTreeMap<String, String> {
        self.launch.env()
    }

    /// Apply `KEY=value` changes: a value sets the variable, an empty one
    /// removes it, and variables not mentioned stay as they are.
    pub fn merge_env(&mut self, changes: BTreeMap<String, String>) {
        let env = self.launch.env_mut();
        for (key, value) in changes {
            if value.is_empty() {
                env.remove(&key);
            } else {
                env.insert(key, value);
            }
        }
    }

    /// One line describing what runs (`npx @google/gemini-cli@0.63.0 --acp`).
    pub fn summary(&self) -> String {
        let (head, args) = match &self.launch {
            AcpLaunch::Command { program, args, .. } => (program.clone(), args),
            AcpLaunch::Npx { package, args, .. } => (format!("npx {package}"), args),
            AcpLaunch::Uvx { package, args, .. } => (format!("uvx {package}"), args),
            AcpLaunch::Binary { cmd, args, .. } => (
                cmd.trim_start_matches("./")
                    .trim_start_matches(".\\")
                    .to_owned(),
                args,
            ),
        };
        std::iter::once(head)
            .chain(args.iter().map(|arg| shell_quote(arg)))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn release(&self) -> Option<RegistryRelease<'_>> {
        match &self.launch {
            AcpLaunch::Binary {
                archive,
                cmd,
                sha256,
                ..
            } => Some(RegistryRelease {
                name: self.id.as_str(),
                version: self.version.as_deref().unwrap_or("current"),
                url: archive,
                entry: binary_entry(cmd),
                sha256: sha256.as_deref(),
            }),
            _ => None,
        }
    }

    pub(crate) fn resolve(&self) -> Result<Resolved, HarnessError> {
        match &self.launch {
            AcpLaunch::Command { program, args, .. } => {
                let path = PathBuf::from(program);
                let found = if path.components().count() > 1 || path.is_absolute() {
                    path.is_file().then_some(path)
                } else {
                    crate::executable::find_on_paths(program, Vec::new())
                };
                found
                    .map(|path| Resolved::Program(path, args.clone()))
                    .ok_or_else(|| {
                        HarnessError::NotInstalled(format!(
                            "{program} (searched PATH and the login shell's PATH)"
                        ))
                    })
            }
            AcpLaunch::Npx { package, args, .. } => {
                let pin = NpmPin::parse(zeron_proto::intern(package));
                let bin_name = zeron_proto::intern(npm_bin_name(pin.name));
                if crate::adapter_install::installed_entry(&pin, bin_name).is_some()
                    || crate::adapter_install::find_npm().is_some()
                {
                    Ok(Resolved::Npm {
                        pin,
                        bin_name,
                        args: args.clone(),
                    })
                } else {
                    Err(HarnessError::NotInstalled(format!(
                        "npm (required to install {}'s package {package})",
                        self.name
                    )))
                }
            }
            AcpLaunch::Uvx { package, args, .. } => {
                let uvx = find_uvx().ok_or_else(|| {
                    HarnessError::NotInstalled(format!(
                        "uvx (required to run {}; install uv from https://docs.astral.sh/uv/)",
                        self.name
                    ))
                })?;
                let mut argv = vec![package.clone()];
                argv.extend(args.iter().cloned());
                Ok(Resolved::Program(uvx, argv))
            }
            AcpLaunch::Binary { args, .. } => {
                let release = self.release().expect("binary launch");
                crate::archive_install::installed_registry_entry(&release)
                    .map(|entry| Resolved::Program(entry, args.clone()))
                    .ok_or_else(|| {
                        HarnessError::NotInstalled(format!(
                            "{} is not downloaded on this device — remove it in Settings → \
                             Providers and add it again",
                            self.name
                        ))
                    })
            }
        }
    }

    /// Ready to launch without installing anything first.
    pub fn installed(&self) -> bool {
        match self.resolve() {
            Ok(Resolved::Program(..)) => true,
            Ok(Resolved::Npm { pin, bin_name, .. }) => {
                crate::adapter_install::installed_entry(&pin, bin_name).is_some()
            }
            Err(_) => false,
        }
    }

    /// Download/install whatever the launch needs.
    pub async fn prepare(&self) -> Result<(), HarnessError> {
        match self.resolve() {
            Ok(Resolved::Npm { pin, bin_name, .. }) => {
                crate::adapter_install::ensure_installed(pin, bin_name, &self.name).await?;
                Ok(())
            }
            Ok(Resolved::Program(..)) => Ok(()),
            Err(error) => match self.release() {
                Some(release) => {
                    crate::archive_install::install_registry(&release, &self.name).await?;
                    Ok(())
                }
                None => Err(error),
            },
        }
    }

    /// Delete the files [`Self::prepare`] downloaded that nothing else uses.
    /// npm installs are keyed by package and may be shared, so they stay.
    pub fn remove_files(&self) {
        if self.release().is_some() {
            crate::archive_install::remove_installs(self.id.as_str());
        }
    }

    /// Static fallback catalog: the agent's own default model.
    pub fn fallback_models() -> Vec<Model> {
        vec![Model {
            id: "default".into(),
            label: "Default".into(),
            description: Some("The agent's own default model".into()),
            reasoning_levels: Vec::new(),
            options: Vec::new(),
        }]
    }
}

/// `./goose-package\goose.exe` → `goose-package/goose.exe`.
fn binary_entry(cmd: &str) -> &str {
    let trimmed = cmd.trim_start_matches("./").trim_start_matches(".\\");
    // Interned so the borrowed release can outlive this call's temporaries.
    zeron_proto::intern(&trimmed.replace('\\', "/"))
}

/// The bin `npx` would pick: the unscoped package name.
fn npm_bin_name(package: &str) -> &str {
    package.rsplit('/').next().unwrap_or(package)
}

fn program_label(program: &str) -> String {
    let base = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let base = base.strip_suffix(".exe").unwrap_or(base);
    let mut chars = base.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => program.to_owned(),
    }
}

/// Lowercase ascii letters and digits joined by single dashes, ≤40 bytes.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    out.trim_end_matches('-').to_owned()
}

fn shell_quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@+,".contains(c))
    {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// Split a command line into words: whitespace separates, single and double
/// quotes group, backslash escapes outside single quotes.
pub fn split_command_line(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => current.push(c),
                        None => return Err("Unclosed ' quote".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c) => current.push(c),
                            None => return Err("Unclosed \" quote".into()),
                        },
                        Some(c) => current.push(c),
                        None => return Err("Unclosed \" quote".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(c) = chars.next() {
                    current.push(c);
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                current.push(c);
            }
        }
    }
    if in_word {
        words.push(current);
    }
    Ok(words)
}

/// `KEY=value` words (as [`split_command_line`] splits them) into a map.
pub fn parse_env_assignments(line: &str) -> Result<BTreeMap<String, String>, String> {
    let mut env = BTreeMap::new();
    for word in split_command_line(line)? {
        let Some((key, value)) = word.split_once('=') else {
            return Err(format!("`{word}` is not KEY=value"));
        };
        let valid = !key.is_empty()
            && !key.starts_with(|c: char| c.is_ascii_digit())
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return Err(format!("`{key}` is not a valid variable name"));
        }
        env.insert(key.to_owned(), value.to_owned());
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = r#"{
        "version": "1.0.0",
        "agents": [
            {
                "id": "gemini", "name": "Gemini CLI", "version": "0.63.0",
                "description": "Google's official CLI for Gemini",
                "icon": "https://cdn.agentclientprotocol.com/registry/v1/latest/gemini.svg",
                "distribution": { "npx": { "package": "@google/gemini-cli@0.63.0", "args": ["--acp"] } }
            },
            {
                "id": "goose", "name": "goose", "version": "1.54.0", "description": "",
                "distribution": { "binary": {
                    "linux-x86_64": { "archive": "https://example.com/goose.tar.bz2", "cmd": "./goose", "args": ["acp"], "sha256": "ab" },
                    "windows-x86_64": { "archive": "https://example.com/goose.zip", "cmd": "./goose-package\\goose.exe", "args": ["acp"] }
                } }
            },
            { "id": "fast-agent", "name": "fast-agent", "version": "0.10.1",
              "distribution": { "uvx": { "package": "fast-agent-acp==0.10.1", "args": ["-x"], "env": { "FAST_AGENT_MODEL": "codexplan" } } } },
            { "id": "Bad Id", "name": "Broken", "version": "1", "distribution": {} },
            { "name": "No id" }
        ]
    }"#;

    #[test]
    fn registry_parse_keeps_valid_agents_sorted_by_name() {
        let agents = parse_registry(INDEX.as_bytes()).unwrap();
        let ids: Vec<&str> = agents.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["fast-agent", "gemini", "goose"]);
        assert_eq!(agents[1].harness_id(), Some(HarnessId::Acp("acp.gemini")));
        assert_eq!(
            agents[2].icon_url(),
            "https://cdn.agentclientprotocol.com/registry/v1/latest/goose.svg"
        );
    }

    #[test]
    fn distribution_prefers_binary_then_npm_then_uv() {
        let agents = parse_registry(INDEX.as_bytes()).unwrap();
        let by_id = |id: &str| agents.iter().find(|a| a.id == id).unwrap().clone();
        let all = Tools {
            platform: Some("linux-x86_64"),
            npm: true,
            uvx: true,
        };
        assert!(matches!(
            choose_distribution(&by_id("goose"), &all),
            Ok(AcpLaunch::Binary { ref cmd, ref sha256, .. }) if cmd == "./goose" && sha256.as_deref() == Some("ab")
        ));
        assert!(matches!(
            choose_distribution(&by_id("gemini"), &all),
            Ok(AcpLaunch::Npx { ref package, .. }) if package == "@google/gemini-cli@0.63.0"
        ));
        let uv = choose_distribution(&by_id("fast-agent"), &all).unwrap();
        assert_eq!(
            uv.env().get("FAST_AGENT_MODEL").map(String::as_str),
            Some("codexplan")
        );
        let bare = Tools {
            platform: Some("darwin-aarch64"),
            npm: false,
            uvx: false,
        };
        assert_eq!(
            choose_distribution(&by_id("goose"), &bare).unwrap_err(),
            "No build for this device"
        );
        assert_eq!(
            choose_distribution(&by_id("gemini"), &bare).unwrap_err(),
            "Needs Node.js (npm) on this device"
        );
        assert_eq!(
            choose_distribution(&by_id("fast-agent"), &bare).unwrap_err(),
            "Needs uv on this device"
        );
    }

    #[test]
    fn windows_entries_normalize_to_relative_paths() {
        assert_eq!(
            binary_entry("./goose-package\\goose.exe"),
            "goose-package/goose.exe"
        );
        assert_eq!(binary_entry("./goose"), "goose");
        assert_eq!(binary_entry("bin/agent"), "bin/agent");
    }

    #[test]
    fn custom_commands_get_stable_ids_and_labels() {
        let agent = AcpAgentConfig::command(
            "",
            "/opt/tools/my-agent",
            vec!["acp".into()],
            BTreeMap::new(),
            None,
        )
        .unwrap();
        assert_eq!(agent.name, "My-agent");
        assert_eq!(agent.id, HarnessId::Acp("acp.custom-my-agent"));
        assert_eq!(agent.summary(), "/opt/tools/my-agent acp");
        let second =
            AcpAgentConfig::command("My Agent!", "agent", Vec::new(), BTreeMap::new(), Some(2))
                .unwrap();
        assert_eq!(second.id.as_str(), "acp.custom-my-agent-2");
        assert!(AcpAgentConfig::command("x", "  ", Vec::new(), BTreeMap::new(), None).is_err());
    }

    #[test]
    fn config_round_trips_through_json() {
        let agents = parse_registry(INDEX.as_bytes()).unwrap();
        let mut config = AcpAgentConfig {
            launch: choose_distribution(
                &agents[1],
                &Tools {
                    platform: None,
                    npm: true,
                    uvx: false,
                },
            )
            .unwrap(),
            ..AcpAgentConfig::command("Gemini CLI", "gemini", Vec::new(), BTreeMap::new(), None)
                .unwrap()
        };
        config.id = agents[1].harness_id().unwrap();
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["id"], "acp.gemini");
        assert_eq!(json["launch"]["kind"], "npx");
        assert_eq!(json["launch"]["package"], "@google/gemini-cli@0.63.0");
        let back: AcpAgentConfig = serde_json::from_value(json).unwrap();
        assert_eq!(back, config);
        assert_eq!(back.summary(), "npx @google/gemini-cli@0.63.0 --acp");
    }

    #[test]
    fn command_lines_split_like_a_shell() {
        assert_eq!(
            split_command_line(r#"agent --acp "two words" 'single \x' a\ b"#).unwrap(),
            ["agent", "--acp", "two words", "single \\x", "a b"]
        );
        assert!(split_command_line("agent 'open").is_err());
        assert_eq!(
            parse_env_assignments("API_KEY=abc MODE='fast mode'").unwrap(),
            BTreeMap::from([
                ("API_KEY".to_string(), "abc".to_string()),
                ("MODE".to_string(), "fast mode".to_string()),
            ])
        );
        assert!(parse_env_assignments("1BAD=x").is_err());
        assert!(parse_env_assignments("novalue").is_err());
    }

    #[test]
    fn env_changes_merge_and_empty_values_remove() {
        let mut agent = AcpAgentConfig::command(
            "a",
            "agent",
            Vec::new(),
            BTreeMap::from([("KEEP".into(), "1".into()), ("DROP".into(), "2".into())]),
            None,
        )
        .unwrap();
        agent.merge_env(parse_env_assignments("DROP= NEW=3 KEEP=1").unwrap());
        assert_eq!(
            agent.env(),
            &BTreeMap::from([
                ("KEEP".to_string(), "1".to_string()),
                ("NEW".to_string(), "3".to_string())
            ])
        );
    }

    #[test]
    fn slugs_are_path_safe() {
        assert_eq!(slug("  Qwen Code (beta) "), "qwen-code-beta");
        assert_eq!(slug("日本"), "");
    }
}
