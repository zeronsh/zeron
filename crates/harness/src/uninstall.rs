//! Explicit, user-requested removal of an agent CLI. Only removes what an
//! installer Zeron runs would have put on disk, recognized by its exact
//! layout: the vendor scripts' launcher + versions directories, a global npm
//! package (removed by npm itself), and Zeron's managed adapters/archives.
//! Anything else — Homebrew, system packages, a hand-placed binary, an
//! `*_EXECUTABLE` override — is refused with the command that removes it.
//! Accounts, credentials and settings (`~/.claude`, `~/.codex/auth.json`,
//! `~/.grok/auth.json`, …) are never touched.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zeron_proto::HarnessId;

use crate::{CancellationToken, Harness, HarnessError, process::Command};

/// npm removes a package in seconds; a stuck registry lock must not pin the
/// agent's install slot forever.
const NPM_DEADLINE: Duration = Duration::from_secs(5 * 60);
/// Bounds the "another copy is still on PATH" loop.
const MAX_COPIES: usize = 4;

/// Why an uninstall did not (fully) happen. Messages are user-facing.
#[derive(Debug, thiserror::Error)]
pub enum UninstallError {
    /// The CLI exists, but not where a Zeron installer puts it.
    #[error("{0}")]
    Outside(String),
    #[error("{0}")]
    NotInstalled(String),
    #[error("{0}")]
    Failed(String),
}

/// What an uninstall removed (or, for a dry run, would remove).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallReport {
    /// Paths (`~`-relative) and commands, in removal order.
    pub removed: Vec<String>,
    /// Another copy still on PATH after Zeron's own was removed, with the
    /// command that removes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

/// One removal. Never follows a symlink out of the path it names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Step {
    /// `npm uninstall -g --prefix <prefix> <package>` with that prefix's npm.
    Npm {
        prefix: PathBuf,
        package: &'static str,
    },
    /// A file or symlink.
    File(PathBuf),
    /// A directory tree the installer owns outright.
    Tree(PathBuf),
    /// A directory removed only if nothing else lives in it.
    Prune(PathBuf),
}

/// Where the harness's CLI comes from right now.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Found {
    Missing,
    /// `*_EXECUTABLE` names it; the user owns that path.
    Override(&'static str, PathBuf),
    Cli(PathBuf),
    /// Zeron's pinned archive (Antigravity) under the managed adapters root.
    Archive(PathBuf),
}

/// The directories the layouts are recognized against.
#[derive(Debug, Clone)]
struct Env {
    home: PathBuf,
    /// `$XDG_DATA_HOME`, else `~/.local/share` (Devin honors it).
    data_home: PathBuf,
    codex_home: PathBuf,
    hermes_home: PathBuf,
    adapters: Option<PathBuf>,
}

impl Env {
    fn current() -> Option<Self> {
        let home = crate::executable::home_dir()?;
        let var = |key: &str| {
            std::env::var_os(key)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Some(Self {
            data_home: var("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share")),
            codex_home: var("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
            hermes_home: var("HERMES_HOME").unwrap_or_else(|| home.join(".hermes")),
            adapters: crate::adapter_install::adapters_root(),
            home,
        })
    }

    fn local_bin(&self, name: &str) -> PathBuf {
        self.home.join(".local").join("bin").join(name)
    }

    /// `~/…` for display; other paths verbatim.
    fn show(&self, path: &Path) -> String {
        match path.strip_prefix(&self.home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        }
    }
}

fn display_name(id: HarnessId) -> &'static str {
    use HarnessId::*;
    match id {
        ClaudeCode => "Claude Code",
        Codex => "Codex",
        Cursor => "Cursor",
        Opencode => "OpenCode",
        Pi => "Pi",
        Grok => "Grok",
        Hermes => "Hermes",
        Devin => "Devin",
        Antigravity => "Antigravity",
        Mock => "Mock",
    }
}

/// The CLI launch override each harness honors (adapter-only overrides such
/// as `PI_ACP_EXECUTABLE` do not name the CLI and are not listed).
fn override_var(id: HarnessId) -> Option<&'static str> {
    use HarnessId::*;
    Some(match id {
        ClaudeCode => "CLAUDE_CODE_EXECUTABLE",
        Codex => "CODEX_EXECUTABLE",
        Opencode => "OPENCODE_EXECUTABLE",
        Grok => "GROK_EXECUTABLE",
        Hermes => "HERMES_EXECUTABLE",
        Devin => "DEVIN_EXECUTABLE",
        Antigravity => "ANTIGRAVITY_ACP_EXECUTABLE",
        Cursor | Pi | Mock => return None,
    })
}

/// Global npm packages that provide each CLI (the installer's npm fallback,
/// plus the historical package names users installed by hand).
fn npm_packages(id: HarnessId) -> &'static [&'static str] {
    use HarnessId::*;
    match id {
        ClaudeCode => &["@anthropic-ai/claude-code"],
        Codex => &["@openai/codex"],
        Opencode => &["@opencode/cli", "opencode-ai"],
        Pi => &["@earendil-works/pi-coding-agent"],
        Grok => &["@xai-official/grok"],
        Cursor | Hermes | Devin | Antigravity | Mock => &[],
    }
}

/// Directories under the managed adapters root that belong to `id`.
fn adapter_dirs(id: HarnessId) -> Vec<String> {
    match id {
        HarnessId::Cursor => {
            vec![crate::adapter_install::NpmPin::parse(crate::cursor::CURSOR_SDK_PIN).dir_name()]
        }
        HarnessId::Antigravity => crate::acp::antigravity_archive()
            .map(|pin| pin.name.to_owned())
            .into_iter()
            .collect(),
        other => crate::acp::managed_adapter_dirs(other),
    }
}

fn locate(id: HarnessId) -> Found {
    if let Some(var) = override_var(id)
        && let Some(path) = std::env::var_os(var).filter(|value| !value.is_empty())
    {
        return Found::Override(var, PathBuf::from(path));
    }
    use HarnessId::*;
    let cli = match id {
        ClaudeCode => crate::ClaudeHarness::new().executable_path(),
        Codex => crate::CodexHarness::new().executable_path(),
        Cursor => crate::CursorHarness::new().executable_path(),
        Opencode => crate::OpencodeHarness::new().executable_path(),
        Pi => crate::AcpHarness::pi().executable_path(),
        Grok => crate::AcpHarness::grok().executable_path(),
        Hermes => crate::AcpHarness::hermes().executable_path(),
        Devin => crate::AcpHarness::devin().executable_path(),
        Antigravity => {
            if let Some(pin) = crate::acp::antigravity_archive()
                && let Some(entry) = crate::archive_install::installed_entry(&pin)
            {
                return Found::Archive(entry);
            }
            crate::AcpHarness::antigravity().executable_path()
        }
        Mock => None,
    };
    cli.map_or(Found::Missing, Found::Cli)
}

fn canon(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// `a` and `b` name the same directory entry (PATH spellings differ).
fn same_entry(a: &Path, b: &Path) -> bool {
    a == b
        || (a.file_name() == b.file_name()
            && a.parent()
                .zip(b.parent())
                .is_some_and(|(x, y)| canon(x) == canon(y)))
}

fn is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
}

fn is_plain_file(path: &Path) -> bool {
    path.symlink_metadata().is_ok_and(|meta| meta.is_file())
}

/// A symlink whose final target lies inside `root`.
fn links_into(link: &Path, root: &Path) -> bool {
    is_symlink(link)
        && std::fs::canonicalize(link).is_ok_and(|target| target.starts_with(canon(root)))
}

/// A symlink whose literal target is `target` (the vendor wrote it verbatim).
fn links_to(link: &Path, target: &Path) -> bool {
    std::fs::read_link(link).is_ok_and(|actual| actual == target)
}

/// Recognize the vendor installer's layout for `exe`. `None` when the CLI
/// is not where that installer puts it.
fn vendor_plan(id: HarnessId, exe: &Path, env: &Env) -> Option<Vec<Step>> {
    use HarnessId::*;
    use Step::*;
    match id {
        // claude.ai/install.sh → `claude install`: ~/.local/bin/claude →
        // ~/.local/share/claude/versions/<v> (Anthropic's documented uninstall
        // removes exactly these two). The older migrate-installer kept a
        // private npm tree in ~/.claude/local.
        ClaudeCode => {
            let launcher = env.local_bin("claude");
            let data = env.home.join(".local/share/claude");
            let local = env.home.join(".claude/local");
            if same_entry(exe, &launcher) && links_into(&launcher, &data.join("versions")) {
                Some(vec![File(launcher), Tree(data)])
            } else if same_entry(exe, &local.join("claude")) {
                Some(vec![Tree(local)])
            } else {
                None
            }
        }
        // chatgpt.com/codex/install.sh: ~/.local/bin/codex →
        // $CODEX_HOME/packages/standalone/current/bin/codex; earlier scripts
        // dropped the binary in $CODEX_HOME/bin.
        Codex => {
            let launcher = env.local_bin("codex");
            let root = env.codex_home.join("packages/standalone");
            let legacy = env.codex_home.join("bin");
            if same_entry(exe, &launcher) && links_into(&launcher, &root) {
                let mut steps = vec![File(launcher)];
                let host = env.local_bin("codex-code-mode-host");
                if links_into(&host, &root) {
                    steps.push(File(host));
                }
                steps.push(Tree(root));
                steps.push(Prune(env.codex_home.join("packages")));
                Some(steps)
            } else if same_entry(exe, &legacy.join("codex")) {
                Some(vec![Tree(legacy)])
            } else {
                None
            }
        }
        // cursor.com/install: ~/.local/bin/{cursor-agent,agent} →
        // ~/.local/share/cursor-agent/versions/<v>/cursor-agent.
        Cursor => {
            let root = env.home.join(".local/share/cursor-agent");
            let names = ["cursor-agent", "agent"];
            if !names
                .iter()
                .any(|name| same_entry(exe, &env.local_bin(name)))
                || !links_into(exe, &root)
            {
                return None;
            }
            let mut steps: Vec<_> = names
                .iter()
                .map(|name| env.local_bin(name))
                .filter(|link| links_into(link, &root))
                .map(File)
                .collect();
            steps.push(Tree(root));
            Some(steps)
        }
        // opencode.ai/install: a single binary in ~/.opencode/bin.
        Opencode => {
            let dir = env.home.join(".opencode/bin");
            let binary = dir.join("opencode");
            (same_entry(exe, &binary) && is_plain_file(&binary))
                .then(|| vec![File(binary), Prune(dir), Prune(env.home.join(".opencode"))])
        }
        // pi.dev/install.sh (managed): <agent>/bin/pi is a launcher script over
        // <agent>/install (marked by managed-install.json), optionally linked
        // from a PATH directory.
        Pi => {
            let launcher = canon(exe);
            let bin = launcher.parent()?;
            let agent = bin.parent()?;
            let install = agent.join("install");
            let managed = launcher.file_name()? == "pi"
                && bin.file_name()? == "bin"
                && agent.starts_with(canon(&env.home))
                && pi_marker_valid(&install);
            if !managed {
                return None;
            }
            let mut steps = Vec::new();
            if is_symlink(exe) {
                steps.push(File(exe.to_path_buf()));
            }
            steps.extend([
                File(launcher.clone()),
                Tree(install),
                Prune(bin.to_path_buf()),
            ]);
            Some(steps)
        }
        // x.ai/cli/install.sh: ~/.grok/bin/{grok,agent} → ../downloads/<bin>,
        // plus convenience links in ~/.local/bin or /usr/local/bin that point
        // at ~/.grok/bin, and generated completions.
        Grok => {
            let grok = env.home.join(".grok");
            let bin = grok.join("bin");
            let downloads = grok.join("downloads");
            let ours = same_entry(exe, &bin.join("grok")) || links_to(exe, &bin.join("grok"));
            if !ours || !links_into(exe, &downloads) {
                return None;
            }
            let mut steps = Vec::new();
            for dir in [env.home.join(".local/bin"), PathBuf::from("/usr/local/bin")] {
                for name in ["grok", "agent"] {
                    if links_to(&dir.join(name), &bin.join(name)) {
                        steps.push(File(dir.join(name)));
                    }
                }
            }
            for name in ["grok", "agent"] {
                if links_into(&bin.join(name), &downloads) {
                    steps.push(File(bin.join(name)));
                }
            }
            steps.extend([Prune(bin), Tree(downloads), Tree(grok.join("completions"))]);
            Some(steps)
        }
        // hermes-agent install.sh: a managed checkout in $HERMES_HOME/hermes-agent
        // with a launcher linked from ~/.local/bin.
        Hermes => {
            let checkout = env.hermes_home.join("hermes-agent");
            if !links_into(exe, &checkout) && !canon(exe).starts_with(canon(&checkout)) {
                return None;
            }
            let mut steps = Vec::new();
            for link in [exe.to_path_buf(), env.local_bin("hermes")] {
                if links_into(&link, &checkout) && !steps.contains(&File(link.clone())) {
                    steps.push(File(link));
                }
            }
            steps.push(Tree(checkout));
            Some(steps)
        }
        // cli.devin.ai/install.sh: ~/.local/bin/devin →
        // $XDG_DATA_HOME/devin/cli/_versions/current/bin/devin (older
        // namespaces: cognition, chisel), plus man-page links.
        Devin => {
            let launcher = env.local_bin("devin");
            if !same_entry(exe, &launcher) {
                return None;
            }
            let versions = ["devin", "cognition", "chisel"]
                .into_iter()
                .map(|ns| env.data_home.join(ns).join("cli/_versions"))
                .find(|versions| links_into(&launcher, versions))?;
            let mut steps = vec![File(launcher)];
            if let Ok(entries) = std::fs::read_dir(env.data_home.join("man/man1")) {
                for entry in entries.flatten() {
                    if links_into(&entry.path(), &versions) {
                        steps.push(File(entry.path()));
                    }
                }
            }
            steps.push(Tree(versions));
            Some(steps)
        }
        Antigravity | Mock => None,
    }
}

fn pi_marker_valid(install: &Path) -> bool {
    std::fs::read_to_string(install.join("managed-install.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .is_some_and(|marker| marker["kind"] == "pi-managed-install")
}

/// `<prefix>/lib/node_modules/<package>/…` (Windows: `<prefix>/node_modules/…`)
/// for one of `packages` → (prefix, package). A project-local node_modules
/// or a Zeron-managed adapter never matches.
fn npm_global(
    canonical: &Path,
    packages: &'static [&'static str],
    env: &Env,
) -> Option<(PathBuf, &'static str)> {
    if env
        .adapters
        .as_ref()
        .is_some_and(|root| canonical.starts_with(canon(root)))
    {
        return None;
    }
    let parts: Vec<OsString> = canonical
        .components()
        .map(|c| c.as_os_str().to_owned())
        .collect();
    let at = parts.iter().position(|part| part == "node_modules")?;
    let package = packages.iter().copied().find(|package| {
        let names: Vec<&str> = package.split('/').collect();
        parts.len() > at + names.len()
            && names
                .iter()
                .zip(&parts[at + 1..])
                .all(|(name, part)| part == name)
    })?;
    let root: PathBuf = parts[..at].iter().collect();
    let prefix = if cfg!(windows) {
        root
    } else if root.file_name()? == "lib" {
        root.parent()?.to_path_buf()
    } else {
        return None;
    };
    Some((prefix, package))
}

/// Everything Zeron would remove for the CLI at `found`.
fn plan(id: HarnessId, found: &Found, env: &Env) -> Result<Vec<Step>, UninstallError> {
    let name = display_name(id);
    match found {
        Found::Missing => Ok(Vec::new()),
        Found::Override(var, path) => Err(UninstallError::Outside(format!(
            "{name} is configured through ${var} ({}), outside Zeron; unset {var} and remove that file yourself",
            path.display()
        ))),
        Found::Archive(entry) => {
            let dir = env
                .adapters
                .as_ref()
                .and_then(|root| {
                    let root = canon(root);
                    let entry = canon(entry);
                    let rest = entry.strip_prefix(&root).ok()?;
                    rest.components().next().map(|first| root.join(first))
                })
                .ok_or_else(|| {
                    UninstallError::Failed(format!(
                        "{name}'s managed install at {} is outside the adapters directory",
                        entry.display()
                    ))
                })?;
            Ok(vec![Step::Tree(dir)])
        }
        Found::Cli(exe) => {
            if let Some((prefix, package)) = npm_global(&canon(exe), npm_packages(id), env) {
                return Ok(vec![Step::Npm { prefix, package }]);
            }
            vendor_plan(id, exe, env).ok_or_else(|| outside(id, exe, env))
        }
    }
}

/// The refusal for a CLI Zeron did not install, naming how to remove it.
fn outside(id: HarnessId, exe: &Path, env: &Env) -> UninstallError {
    let canonical = canon(exe);
    let text = format!("{}\n{}", exe.display(), canonical.display())
        .replace('\\', "/")
        .to_ascii_lowercase();
    let how = if text.contains("/cellar/")
        || text.contains("/caskroom/")
        || text.contains("/opt/homebrew/")
        || text.contains("/linuxbrew/")
    {
        match id {
            HarnessId::ClaudeCode => "`brew uninstall --cask claude-code`".into(),
            HarnessId::Codex => "`brew uninstall --cask codex`".into(),
            HarnessId::Devin => "`brew uninstall --cask devin-cli`".into(),
            HarnessId::Opencode => "`brew uninstall opencode`".into(),
            _ => "Homebrew (`brew uninstall` the formula that provides it)".into(),
        }
    } else if ["/usr/bin/", "/bin/", "/usr/lib/", "/nix/store/", "/snap/"]
        .iter()
        .any(|root| canonical.to_string_lossy().starts_with(root))
    {
        "the system package manager that installed it".into()
    } else if text.contains("/node_modules/") {
        "the package manager that installed it".into()
    } else {
        format!("`rm {}`", env.show(exe))
    };
    UninstallError::Outside(format!(
        "{} is installed outside Zeron ({}); remove it with {how}",
        display_name(id),
        env.show(exe)
    ))
}

fn describe(step: &Step, env: &Env) -> String {
    match step {
        Step::Npm { prefix, package } => {
            format!("npm uninstall -g {package} ({})", env.show(prefix))
        }
        Step::File(path) | Step::Tree(path) | Step::Prune(path) => env.show(path),
    }
}

fn npm_command(prefix: &Path, package: &str) -> Result<Command, UninstallError> {
    let owned = prefix.join("bin").join("npm");
    let npm = if !cfg!(windows) && owned.is_file() {
        owned
    } else {
        crate::adapter_install::find_npm().ok_or_else(|| {
            UninstallError::Failed(format!(
                "npm is needed to remove {package} but was not found on this device"
            ))
        })?
    };
    let platform = crate::executable::Platform::current();
    let mut command = if platform == crate::executable::Platform::Windows {
        let node =
            crate::adapter_install::node_sibling_for_npm(&npm, platform).ok_or_else(|| {
                UninstallError::Failed(format!("node.exe beside {} was not found", npm.display()))
            })?;
        let mut command = Command::new(node);
        command.arg(&npm);
        command
    } else {
        Command::new(&npm)
    };
    command
        .args([
            "uninstall",
            "-g",
            "--no-audit",
            "--no-fund",
            "--loglevel=error",
            "--prefix",
        ])
        .arg(prefix)
        .arg(package);
    crate::install::configure(&mut command);
    // npm's shebang needs the node that owns this prefix first on PATH.
    if let Some(dir) = npm.parent() {
        let rest = std::env::var_os("PATH").unwrap_or_default();
        if let Ok(path) = std::env::join_paths(
            std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&rest)),
        ) {
            command.env("PATH", path);
        }
    }
    Ok(command)
}

fn remove(path: &Path) -> std::io::Result<()> {
    let meta = match path.symlink_metadata() {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Run `steps`; returns what was actually removed. npm runs first so a
/// failure or cancellation there leaves every file untouched.
async fn execute(
    steps: &[Step],
    env: &Env,
    cancel: &CancellationToken,
) -> Result<Vec<String>, UninstallError> {
    let mut removed = Vec::new();
    for step in steps.iter().filter(|s| matches!(s, Step::Npm { .. })) {
        let Step::Npm { prefix, package } = step else {
            continue;
        };
        let labels = crate::install::Labels {
            noun: "uninstall",
            actor: "npm uninstall",
        };
        crate::install::run(
            npm_command(prefix, package)?,
            cancel.clone(),
            NPM_DEADLINE,
            labels,
        )
        .await
        .map_err(|error| match error {
            HarnessError::Install(message) => UninstallError::Failed(message),
            other => UninstallError::Failed(other.to_string()),
        })?;
        removed.push(describe(step, env));
    }
    if cancel.is_cancelled() {
        return Err(UninstallError::Failed("uninstall cancelled".into()));
    }
    for step in steps {
        let fail = |error: std::io::Error| {
            UninstallError::Failed(format!("could not remove {}: {error}", describe(step, env)))
        };
        match step {
            Step::Npm { .. } => {}
            Step::File(path) | Step::Tree(path) => {
                if path.symlink_metadata().is_ok() {
                    remove(path).map_err(fail)?;
                    removed.push(describe(step, env));
                }
            }
            Step::Prune(path) => {
                if std::fs::remove_dir(path).is_ok() {
                    removed.push(describe(step, env));
                }
            }
        }
    }
    Ok(removed)
}

/// The managed adapter directories that exist for `id`.
fn adapter_steps(id: HarnessId, env: &Env) -> Vec<Step> {
    let Some(root) = &env.adapters else {
        return Vec::new();
    };
    adapter_dirs(id)
        .into_iter()
        .map(|dir| root.join(dir))
        .filter(|dir| dir.symlink_metadata().is_ok())
        .map(Step::Tree)
        .collect()
}

/// What [`uninstall_harness`] would remove, without touching anything.
pub fn preview(id: HarnessId) -> Result<UninstallReport, UninstallError> {
    let env = Env::current()
        .ok_or_else(|| UninstallError::Failed("HOME is not set on this device".into()))?;
    let mut steps = plan(id, &locate(id), &env)?;
    steps.extend(adapter_steps(id, &env));
    if steps.is_empty() {
        return Err(not_installed(id));
    }
    Ok(UninstallReport {
        removed: steps
            .iter()
            .filter(|step| !matches!(step, Step::Prune(_)))
            .map(|step| describe(step, &env))
            .collect(),
        remaining: None,
        dry_run: true,
    })
}

fn not_installed(id: HarnessId) -> UninstallError {
    UninstallError::NotInstalled(format!(
        "{} is not installed on this device",
        display_name(id)
    ))
}

/// Only the explicit Uninstall RPC may call this. Removes every copy of the
/// CLI Zeron can attribute (vendor layout, global npm package, managed
/// archive), then its managed adapters. A copy outside Zeron is refused when
/// it is the only one, or reported in [`UninstallReport::remaining`] once
/// Zeron's own copy is gone.
pub async fn uninstall_harness(
    id: HarnessId,
    cancel: CancellationToken,
) -> Result<UninstallReport, UninstallError> {
    let env = Env::current()
        .ok_or_else(|| UninstallError::Failed("HOME is not set on this device".into()))?;
    let result = uninstall_in(id, &env, &cancel, locate).await;
    crate::install::invalidate_versions(id);
    result
}

async fn uninstall_in(
    id: HarnessId,
    env: &Env,
    cancel: &CancellationToken,
    locate: impl Fn(HarnessId) -> Found,
) -> Result<UninstallReport, UninstallError> {
    let mut report = UninstallReport::default();
    let mut seen = HashSet::new();
    for _ in 0..MAX_COPIES {
        let found = locate(id);
        let steps = match plan(id, &found, env) {
            Ok(steps) if steps.is_empty() => break,
            Ok(steps) => steps,
            Err(refusal) if report.removed.is_empty() => return Err(refusal),
            Err(refusal) => {
                report.remaining = Some(refusal.to_string());
                break;
            }
        };
        if !seen.insert(steps.clone()) {
            // Removal reported success but the same copy still resolves.
            report.remaining = Some(format!(
                "{} is still installed after removing {}",
                display_name(id),
                steps
                    .first()
                    .map(|step| describe(step, env))
                    .unwrap_or_default()
            ));
            break;
        }
        report.removed.extend(execute(&steps, env, cancel).await?);
        crate::install::invalidate_versions(id);
    }
    report
        .removed
        .extend(execute(&adapter_steps(id, env), env, cancel).await?);
    if report.removed.is_empty() {
        return Err(not_installed(id));
    }
    Ok(report)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    struct Fixture {
        _dir: tempfile::TempDir,
        env: Env,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap().join("home");
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        let env = Env {
            data_home: home.join(".local/share"),
            codex_home: home.join(".codex"),
            hermes_home: home.join(".hermes"),
            adapters: Some(home.join(".zeron/adapters")),
            home,
        };
        Fixture { _dir: dir, env }
    }

    fn file(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn link(target: &Path, at: &Path) {
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        symlink(target, at).unwrap();
    }

    fn gone(path: &Path) -> bool {
        path.symlink_metadata().is_err()
    }

    async fn run(
        id: HarnessId,
        env: &Env,
        found: Found,
    ) -> Result<UninstallReport, UninstallError> {
        // After one removal the CLI no longer resolves.
        let first = std::sync::Mutex::new(Some(found));
        uninstall_in(id, env, &CancellationToken::new(), |_| {
            first.lock().unwrap().take().unwrap_or(Found::Missing)
        })
        .await
    }

    #[tokio::test]
    async fn claude_native_installer_layout_is_removed_and_credentials_kept() {
        let f = fixture();
        let home = &f.env.home;
        let version = home.join(".local/share/claude/versions/2.1.14");
        file(&version, "#!/bin/sh\necho 2.1.14\n");
        let launcher = home.join(".local/bin/claude");
        link(&version, &launcher);
        file(&home.join(".claude/.credentials.json"), "{}");
        file(&home.join(".claude.json"), "{}");

        let report = run(HarnessId::ClaudeCode, &f.env, Found::Cli(launcher.clone()))
            .await
            .unwrap();
        assert_eq!(
            report.removed,
            ["~/.local/bin/claude", "~/.local/share/claude"]
        );
        assert!(gone(&launcher) && gone(&home.join(".local/share/claude")));
        assert!(home.join(".claude/.credentials.json").is_file());
        assert!(home.join(".claude.json").is_file());
    }

    #[tokio::test]
    async fn codex_standalone_layout_keeps_codex_home() {
        let f = fixture();
        let home = &f.env.home;
        let root = home.join(".codex/packages/standalone");
        let release = root.join("releases/0.42.0-x86_64-unknown-linux-musl");
        file(&release.join("bin/codex"), "#!/bin/sh\n");
        link(&release, &root.join("current"));
        let launcher = home.join(".local/bin/codex");
        link(&root.join("current/bin/codex"), &launcher);
        file(&home.join(".codex/auth.json"), "{}");

        let report = run(HarnessId::Codex, &f.env, Found::Cli(launcher.clone()))
            .await
            .unwrap();
        assert!(
            report
                .removed
                .contains(&"~/.codex/packages/standalone".into())
        );
        assert!(gone(&launcher) && gone(&root));
        assert!(home.join(".codex/auth.json").is_file());
    }

    #[tokio::test]
    async fn opencode_grok_cursor_devin_hermes_pi_vendor_layouts() {
        let f = fixture();
        let home = f.env.home.clone();

        // OpenCode: a plain binary in ~/.opencode/bin; config elsewhere stays.
        let opencode = home.join(".opencode/bin/opencode");
        file(&opencode, "bin");
        file(&home.join(".local/share/opencode/auth.json"), "{}");
        let report = run(HarnessId::Opencode, &f.env, Found::Cli(opencode.clone()))
            .await
            .unwrap();
        assert_eq!(
            report.removed,
            ["~/.opencode/bin/opencode", "~/.opencode/bin", "~/.opencode"]
        );
        assert!(home.join(".local/share/opencode/auth.json").is_file());

        // Grok: relative links into downloads, a ~/.local/bin convenience link,
        // and a foreign `agent` link (Cursor's) that must survive.
        let grok = home.join(".grok");
        file(&grok.join("downloads/grok-1.0.5"), "bin");
        link(Path::new("../downloads/grok-1.0.5"), &grok.join("bin/grok"));
        link(
            Path::new("../downloads/grok-1.0.5"),
            &grok.join("bin/agent"),
        );
        link(&grok.join("bin/grok"), &home.join(".local/bin/grok"));
        file(&grok.join("auth.json"), "{}");
        let cursor_version = home.join(".local/share/cursor-agent/versions/2026.09/cursor-agent");
        file(&cursor_version, "bin");
        link(&cursor_version, &home.join(".local/bin/agent"));
        let report = run(
            HarnessId::Grok,
            &f.env,
            Found::Cli(home.join(".local/bin/grok")),
        )
        .await
        .unwrap();
        assert!(
            report.removed.contains(&"~/.grok/downloads".into()),
            "{report:?}"
        );
        assert!(gone(&home.join(".local/bin/grok")) && gone(&grok.join("bin")));
        assert!(grok.join("auth.json").is_file());
        assert!(
            is_symlink(&home.join(".local/bin/agent")),
            "Cursor's link kept"
        );

        // Cursor: both launcher links and the versions tree.
        link(&cursor_version, &home.join(".local/bin/cursor-agent"));
        run(
            HarnessId::Cursor,
            &f.env,
            Found::Cli(home.join(".local/bin/cursor-agent")),
        )
        .await
        .unwrap();
        assert!(gone(&home.join(".local/bin/agent")));
        assert!(gone(&home.join(".local/share/cursor-agent")));

        // Devin: XDG versions tree + launcher + man link; credentials stay.
        let versions = home.join(".local/share/devin/cli/_versions");
        file(&versions.join("2026.1/bin/devin"), "bin");
        file(&versions.join("2026.1/share/man/man1/devin.1"), "man");
        link(&versions.join("2026.1"), &versions.join("current"));
        link(
            &versions.join("current/bin/devin"),
            &home.join(".local/bin/devin"),
        );
        link(
            &versions.join("current/share/man/man1/devin.1"),
            &home.join(".local/share/man/man1/devin.1"),
        );
        file(&home.join(".local/share/devin/credentials.json"), "{}");
        run(
            HarnessId::Devin,
            &f.env,
            Found::Cli(home.join(".local/bin/devin")),
        )
        .await
        .unwrap();
        assert!(gone(&versions) && gone(&home.join(".local/share/man/man1/devin.1")));
        assert!(home.join(".local/share/devin/credentials.json").is_file());

        // Hermes: the managed checkout; ~/.hermes config stays.
        let checkout = home.join(".hermes/hermes-agent");
        file(&checkout.join(".hermes/bin/hermes"), "bin");
        link(
            &checkout.join(".hermes/bin/hermes"),
            &home.join(".local/bin/hermes"),
        );
        file(&home.join(".hermes/.env"), "KEY=1");
        run(
            HarnessId::Hermes,
            &f.env,
            Found::Cli(home.join(".local/bin/hermes")),
        )
        .await
        .unwrap();
        assert!(gone(&checkout) && gone(&home.join(".local/bin/hermes")));
        assert!(home.join(".hermes/.env").is_file());

        // Pi (managed): PATH link → launcher over a marked install root.
        let agent = home.join(".pi/agent");
        file(
            &agent.join("install/managed-install.json"),
            r#"{"kind":"pi-managed-install","schemaVersion":1,"layout":"releases-v1"}"#,
        );
        file(&agent.join("bin/pi"), "#!/bin/sh\n");
        file(&agent.join("auth.json"), "{}");
        link(&agent.join("bin/pi"), &home.join(".local/bin/pi"));
        run(
            HarnessId::Pi,
            &f.env,
            Found::Cli(home.join(".local/bin/pi")),
        )
        .await
        .unwrap();
        assert!(gone(&home.join(".local/bin/pi")) && gone(&agent.join("install")));
        assert!(agent.join("auth.json").is_file());
    }

    #[tokio::test]
    async fn npm_global_package_is_removed_by_its_own_npm() {
        let f = fixture();
        let prefix = f.env.home.join(".npm-global");
        let package = prefix.join("lib/node_modules/@opencode/cli");
        file(&package.join("bin/opencode"), "#!/usr/bin/env node\n");
        link(&package.join("bin/opencode"), &prefix.join("bin/opencode"));
        let log = f.env.home.join("npm.log");
        // The prefix's own npm; it records its argv and removes the package.
        file(
            &prefix.join("bin/npm"),
            &format!(
                "#!/bin/sh\necho \"$@\" > '{}'\nfor last; do :; done\nrm -rf \"$7/lib/node_modules/$last\" \"$7/bin/opencode\"\n",
                log.display()
            ),
        );
        let report = run(
            HarnessId::Opencode,
            &f.env,
            Found::Cli(prefix.join("bin/opencode")),
        )
        .await
        .unwrap();
        assert_eq!(
            report.removed,
            ["npm uninstall -g @opencode/cli (~/.npm-global)"]
        );
        let argv = std::fs::read_to_string(&log).unwrap();
        assert!(
            argv.starts_with("uninstall -g ")
                && argv.contains(&format!("--prefix {}", prefix.display())),
            "{argv}"
        );
        assert!(gone(&package));

        // A project-local node_modules is not a global install.
        let local = f
            .env
            .home
            .join("proj/node_modules/@opencode/cli/bin/opencode");
        file(&local, "x");
        assert!(npm_global(&local, npm_packages(HarnessId::Opencode), &f.env).is_none());
    }

    #[tokio::test]
    async fn failed_npm_uninstall_reports_and_touches_nothing_else() {
        let f = fixture();
        let prefix = f.env.home.join("npm");
        let package = prefix.join("lib/node_modules/@openai/codex");
        file(&package.join("bin/codex.js"), "x");
        link(&package.join("bin/codex.js"), &prefix.join("bin/codex"));
        file(
            &prefix.join("bin/npm"),
            "#!/bin/sh\necho 'npm error EACCES' >&2\nexit 243\n",
        );
        let error = run(
            HarnessId::Codex,
            &f.env,
            Found::Cli(prefix.join("bin/codex")),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("npm uninstall exited") && error.contains("EACCES"),
            "{error}"
        );
        assert!(package.join("bin/codex.js").is_file());
    }

    #[tokio::test]
    async fn managed_archive_and_adapters_are_removed() {
        let f = fixture();
        let root = f.env.adapters.clone().unwrap();
        let entry = root.join("antigravity-acp/1.1.1/agy_acp_server.par");
        file(&entry, "bin");
        let report = run(HarnessId::Antigravity, &f.env, Found::Archive(entry))
            .await
            .unwrap();
        assert_eq!(report.removed.len(), 1, "{report:?}");
        assert!(gone(&root.join("antigravity-acp")));

        // Grok's managed ACP adapter goes with the CLI (and alone, if the
        // CLI is already gone).
        let adapter = root.join(&adapter_dirs(HarnessId::Grok)[0]);
        file(&adapter.join("1.0.4/.zeron-install-ok"), "1.0.4");
        let report = run(HarnessId::Grok, &f.env, Found::Missing).await.unwrap();
        assert_eq!(report.removed.len(), 1);
        assert!(gone(&adapter));
        assert!(matches!(
            run(HarnessId::Grok, &f.env, Found::Missing).await,
            Err(UninstallError::NotInstalled(_))
        ));
    }

    #[tokio::test]
    async fn refuses_binaries_outside_zeron_install_locations() {
        let f = fixture();
        let home = f.env.home.clone();
        // A hand-placed binary in ~/.local/bin (not the vendor's symlink).
        let loose = home.join(".local/bin/claude");
        file(&loose, "bin");
        let error = run(HarnessId::ClaudeCode, &f.env, Found::Cli(loose.clone()))
            .await
            .unwrap_err();
        assert!(matches!(error, UninstallError::Outside(_)));
        assert!(
            error.to_string().contains("installed outside Zeron"),
            "{error}"
        );
        assert!(
            error.to_string().contains("`rm ~/.local/bin/claude`"),
            "{error}"
        );
        assert!(loose.is_file(), "nothing removed");

        // A Zeron-managed adapter is not removed when the CLI is refused.
        let adapter = f
            .env
            .adapters
            .clone()
            .unwrap()
            .join(&adapter_dirs(HarnessId::Pi)[0]);
        file(&adapter.join("0.0.33/.zeron-install-ok"), "0.0.33");
        let pi = home.join("bin/pi");
        file(&pi, "bin");
        assert!(run(HarnessId::Pi, &f.env, Found::Cli(pi)).await.is_err());
        assert!(adapter.is_dir());

        // Overrides and Homebrew get the command that removes them.
        let error = plan(
            HarnessId::Codex,
            &Found::Override("CODEX_EXECUTABLE", PathBuf::from("/opt/tools/codex")),
            &f.env,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("$CODEX_EXECUTABLE"), "{error}");
        let brew = outside(
            HarnessId::ClaudeCode,
            Path::new("/opt/homebrew/Caskroom/claude-code/2.1.0/claude"),
            &f.env,
        )
        .to_string();
        assert!(brew.contains("brew uninstall --cask claude-code"), "{brew}");
        let system = outside(HarnessId::Codex, Path::new("/usr/bin/codex"), &f.env).to_string();
        assert!(system.contains("system package manager"), "{system}");
    }

    #[tokio::test]
    async fn a_second_copy_outside_zeron_is_reported_not_removed() {
        let f = fixture();
        let home = f.env.home.clone();
        let opencode = home.join(".opencode/bin/opencode");
        file(&opencode, "bin");
        let other = PathBuf::from("/opt/homebrew/bin/opencode");
        let copies = std::sync::Mutex::new(vec![Found::Cli(other), Found::Cli(opencode.clone())]);
        let report = uninstall_in(
            HarnessId::Opencode,
            &f.env,
            &CancellationToken::new(),
            |_| copies.lock().unwrap().pop().unwrap_or(Found::Missing),
        )
        .await
        .unwrap();
        assert!(gone(&opencode));
        let remaining = report.remaining.unwrap();
        assert!(remaining.contains("brew uninstall opencode"), "{remaining}");
    }

    #[tokio::test]
    async fn cancelled_uninstall_leaves_files() {
        let f = fixture();
        let binary = f.env.home.join(".opencode/bin/opencode");
        file(&binary, "bin");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let steps = plan(HarnessId::Opencode, &Found::Cli(binary.clone()), &f.env).unwrap();
        assert!(execute(&steps, &f.env, &cancel).await.is_err());
        assert!(binary.is_file());
    }
}
