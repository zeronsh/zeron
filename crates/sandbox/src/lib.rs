//! OS sandboxes for agent processes (docs/sandbox.md; plan:
//! docs/plans/2026-09-30-agent-mobility-and-policy.md, Part 4).
//!
//! The uniform layer is the *policy* — [`SandboxMode`] plus "network on/off"
//! from [`zeron_proto::AgentPolicy`] — turned into a [`SandboxSpec`]. Each OS
//! enforces that spec with its native mechanism:
//!
//! - **macOS:** Seatbelt (`/usr/bin/sandbox-exec` with a generated SBPL
//!   profile, the mechanism Codex and Claude Code use for their tools).
//! - **Linux:** bubblewrap when `bwrap` is installed and works, else
//!   Landlock (+ seccomp for the network) applied by a helper mode of the
//!   Zeron binary that confines itself and then `execve`s the agent.
//! - **Windows:** not yet; [`wrap`] reports [`SandboxError::Unsupported`].
//!
//! A harness never talks to a backend directly: it builds its command as
//! usual, passes program + args through [`wrap`], and spawns what comes back.
//! The returned [`Enforcement`] says which parts of the spec the backend could
//! actually enforce on this machine, so the UI can say so instead of
//! pretending.

mod bwrap;
// The planning half is unit-tested everywhere; only Linux applies it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod landlock;
mod paths;
mod seatbelt;

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
pub use zeron_proto::SandboxMode;
use zeron_proto::{AgentPolicy, HarnessId};

pub use paths::{
    AgentPaths, Platform, agent_paths_for, canonicalize_lossy, default_agent_paths, git_paths,
    system_temp_dirs, toolchain_cache_paths,
};

/// Everything a backend needs to confine one agent process.
///
/// Precedence when entries overlap: `hidden` beats `read_only` beats every
/// writable entry. Reading is allowed everywhere that is not `hidden`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxSpec {
    pub mode: SandboxMode,
    /// The agent may reach the network. When `false` it may still reach
    /// loopback (see `loopback_ports`).
    pub network: bool,
    /// The project folder; writable under [`SandboxMode::WorkspaceWrite`].
    pub workspace: PathBuf,
    /// Directories the agent needs to write to function in every mode: its
    /// own state (`~/.claude`, `~/.codex`, …). Temp folders are always added
    /// by the backend (see [`system_temp_dirs`]). Landlock and bubblewrap
    /// create missing ones before the spawn, because neither can grant a path
    /// that doesn't exist yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writable: Vec<PathBuf>,
    /// Files written by atomic replace: the file itself and every sibling
    /// whose name starts with the file's name (`~/.claude.json`,
    /// `~/.claude.json.tmp.123.abcd`, `~/.claude.json.lock`). Seatbelt
    /// enforces exactly that; Landlock and bubblewrap can only grant the
    /// existing file (see [`Enforcement::notes`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writable_prefixes: Vec<PathBuf>,
    /// Paths inside writable areas that stay read-only: places where a write
    /// would run code *outside* the sandbox later (`.git/hooks`,
    /// `.git/config`, the agent's own binary and hook config).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_only: Vec<PathBuf>,
    /// Paths the agent may neither read nor write: secret stores (`~/.ssh`,
    /// `~/.aws`, other agents' credentials, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<PathBuf>,
    /// Loopback TCP ports the agent must reach when `network` is off — at
    /// least the engine's IPC port, which the `zeron mcp` server the agent
    /// spawns dials. Seatbelt allows all of loopback anyway; Linux can only
    /// allow these ports (Landlock filters TCP by port, not address).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loopback_ports: Vec<u16>,
}

impl SandboxSpec {
    pub fn new(mode: SandboxMode, network: bool, workspace: impl Into<PathBuf>) -> Self {
        Self {
            mode,
            network,
            workspace: workspace.into(),
            writable: Vec::new(),
            writable_prefixes: Vec::new(),
            read_only: Vec::new(),
            hidden: Vec::new(),
            loopback_ports: Vec::new(),
        }
    }

    /// The sandbox half of a chat's policy.
    pub fn from_policy(policy: &AgentPolicy, workspace: impl Into<PathBuf>) -> Self {
        Self::new(policy.sandbox, policy.network, workspace)
    }

    /// The spec a harness spawn site wants: the policy, the agent's own
    /// paths ([`default_agent_paths`]) and the workspace's git protections
    /// ([`git_paths`]).
    pub fn for_agent(
        harness: HarnessId,
        policy: &AgentPolicy,
        workspace: impl Into<PathBuf>,
        home: &Path,
    ) -> Self {
        let workspace = workspace.into();
        let mut git = git_paths(&workspace);
        if policy.sandbox != SandboxMode::WorkspaceWrite {
            git.writable.clear();
        }
        Self::from_policy(policy, workspace)
            .with_paths(default_agent_paths(harness, home))
            .with_paths(git)
    }

    /// Adds `paths` (its `env` is the caller's to apply).
    pub fn with_paths(mut self, paths: AgentPaths) -> Self {
        self.writable.extend(paths.writable);
        self.writable_prefixes.extend(paths.writable_prefixes);
        self.read_only.extend(paths.read_only);
        self.hidden.extend(paths.hidden);
        self
    }

    pub fn with_loopback_port(mut self, port: u16) -> Self {
        if !self.loopback_ports.contains(&port) {
            self.loopback_ports.push(port);
        }
        self
    }
}

/// An OS mechanism that can enforce a [`SandboxSpec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Backend {
    /// No confinement: the command runs as given.
    None,
    /// macOS `sandbox-exec` with a generated SBPL profile.
    Seatbelt,
    /// Linux Landlock (+ seccomp), applied by the helper mode before `execve`.
    Landlock,
    /// Linux bubblewrap (`bwrap`): mount namespace with read-only root.
    Bubblewrap,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::None => "No sandbox",
            Backend::Seatbelt => "Seatbelt",
            Backend::Landlock => "Landlock",
            Backend::Bubblewrap => "Bubblewrap",
        }
    }

    fn env_value(self) -> &'static str {
        match self {
            Backend::None => "none",
            Backend::Seatbelt => "seatbelt",
            Backend::Landlock => "landlock",
            Backend::Bubblewrap => "bubblewrap",
        }
    }
}

/// What a backend could enforce of the spec it was given, on this machine.
/// A `true` field means "enforced, or nothing was asked".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Enforcement {
    pub backend: Backend,
    /// Writes outside the workspace (WorkspaceWrite), `writable` and temp are
    /// refused.
    pub writes: bool,
    /// `hidden` paths can't be read.
    pub hidden: bool,
    /// `read_only` carve-outs inside writable areas hold.
    pub read_only: bool,
    /// `network: false` holds (always `true` when the network is allowed).
    pub network: bool,
    /// Human-readable caveats, for logs and the policy UI.
    pub notes: Vec<String>,
}

impl Enforcement {
    /// Every part of the spec holds (notes may still describe nuances).
    pub fn is_complete(&self) -> bool {
        self.writes && self.hidden && self.read_only && self.network
    }

    fn unconfined(spec: &SandboxSpec) -> Self {
        Self {
            backend: Backend::None,
            writes: true,
            hidden: true,
            read_only: true,
            network: spec.network,
            notes: if spec.network {
                Vec::new()
            } else {
                vec!["the sandbox is off, so network off is not enforced".into()]
            },
        }
    }
}

/// The command to spawn instead of the original one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrapped {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Added to the child's environment (`ZERON_SANDBOX=<backend>`, so tools
    /// inside can tell they are confined).
    pub env: Vec<(OsString, OsString)>,
    pub enforcement: Enforcement,
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("sandboxing agents is not supported on {0} yet")]
    Unsupported(&'static str),
    #[error("the {} sandbox is not available on this machine", .0.label())]
    BackendUnavailable(Backend),
    #[error("sandbox paths must be absolute: {}", .0.display())]
    RelativePath(PathBuf),
    #[error("sandbox path can't be expressed in a sandbox profile: {}", .0.display())]
    UnsupportedPath(PathBuf),
    #[error("couldn't encode the sandbox spec: {0}")]
    Spec(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// How Landlock (and bubblewrap's network half) re-enter Zeron to confine
/// themselves: `<program> <args…> --spec <json> [--net-only] -- <exe> <args…>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Helper {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// The subcommand the `zeron` binary answers as the helper
/// (see [`run_helper_if_requested`]).
pub const HELPER_SUBCOMMAND: &str = "sandbox-exec";

impl Helper {
    /// `<current exe> sandbox-exec`.
    pub fn current_exe() -> std::io::Result<Self> {
        Ok(Self {
            program: std::env::current_exe()?,
            args: vec![HELPER_SUBCOMMAND.into()],
        })
    }
}

/// Backends usable on this machine, most complete first.
pub fn available() -> Vec<Backend> {
    let mut out = Vec::new();
    if cfg!(target_os = "macos") && seatbelt::available() {
        out.push(Backend::Seatbelt);
    }
    if cfg!(target_os = "linux") {
        if bwrap::available() {
            out.push(Backend::Bubblewrap);
        }
        if landlock::kernel_abi() >= 1 {
            out.push(Backend::Landlock);
        }
    }
    out
}

/// The backend [`wrap`] uses: the first of [`available`], or `None`.
pub fn best_backend() -> Backend {
    available().into_iter().next().unwrap_or(Backend::None)
}

/// Rewrites `program args` so the child runs confined by `spec`, using
/// [`best_backend`] and [`Helper::current_exe`].
///
/// `SandboxMode::Off` returns the command unchanged. Any other mode on a
/// machine without a backend is an error, so the caller decides whether to
/// refuse the run or run it unconfined and say so.
pub fn wrap(
    spec: &SandboxSpec,
    program: &Path,
    args: &[OsString],
) -> Result<Wrapped, SandboxError> {
    if spec.mode == SandboxMode::Off {
        return Ok(passthrough(spec, program, args));
    }
    if cfg!(windows) {
        return Err(SandboxError::Unsupported("Windows"));
    }
    let backend = best_backend();
    if backend == Backend::None {
        return Err(SandboxError::BackendUnavailable(
            if cfg!(target_os = "macos") {
                Backend::Seatbelt
            } else {
                Backend::Landlock
            },
        ));
    }
    let helper = Helper::current_exe()?;
    wrap_with(backend, spec, program, args, &helper)
}

/// [`wrap`] with an explicit backend and helper.
pub fn wrap_with(
    backend: Backend,
    spec: &SandboxSpec,
    program: &Path,
    args: &[OsString],
    helper: &Helper,
) -> Result<Wrapped, SandboxError> {
    if spec.mode == SandboxMode::Off || backend == Backend::None {
        return Ok(passthrough(spec, program, args));
    }
    if cfg!(windows) {
        return Err(SandboxError::Unsupported("Windows"));
    }
    if !available().contains(&backend) {
        return Err(SandboxError::BackendUnavailable(backend));
    }
    let resolved = Resolved::new(spec, system_temp_dirs())?;
    let env = vec![(
        OsString::from("ZERON_SANDBOX"),
        OsString::from(backend.env_value()),
    )];
    let (program, args, enforcement) = match backend {
        Backend::Seatbelt => {
            let profile = seatbelt::profile(&resolved)?;
            let mut out = vec![OsString::from("-p"), OsString::from(profile), "--".into()];
            out.push(program.as_os_str().to_owned());
            out.extend(args.iter().cloned());
            (
                PathBuf::from(seatbelt::SANDBOX_EXEC),
                out,
                seatbelt::enforcement(&resolved),
            )
        }
        Backend::Landlock => {
            resolved.create_missing_writable_dirs();
            let abi = landlock::kernel_abi();
            let mut out = helper.args.clone();
            out.extend(helper_args(spec, false, program, args)?);
            (
                helper.program.clone(),
                out,
                landlock::enforcement(&resolved, abi),
            )
        }
        Backend::Bubblewrap => {
            resolved.create_missing_writable_dirs();
            let abi = landlock::kernel_abi();
            let net = bwrap::network_plan(&resolved, abi);
            let mut inner = Vec::new();
            if net == bwrap::NetworkPlan::Helper {
                inner.push(helper.program.as_os_str().to_owned());
                inner.extend(helper.args.iter().cloned());
                inner.extend(helper_args(spec, true, program, args)?);
            } else {
                inner.push(program.as_os_str().to_owned());
                inner.extend(args.iter().cloned());
            }
            let bwrap_args = bwrap::args(&resolved, &bwrap::host_kind, net, inner);
            (
                bwrap::program().unwrap_or_else(|| PathBuf::from("bwrap")),
                bwrap_args,
                bwrap::enforcement(&resolved, net),
            )
        }
        Backend::None => unreachable!(),
    };
    Ok(Wrapped {
        program,
        args,
        env,
        enforcement,
    })
}

fn passthrough(spec: &SandboxSpec, program: &Path, args: &[OsString]) -> Wrapped {
    Wrapped {
        program: program.to_path_buf(),
        args: args.to_vec(),
        env: Vec::new(),
        enforcement: Enforcement::unconfined(spec),
    }
}

/// `--spec <json> [--net-only] -- <program> <args…>`.
fn helper_args(
    spec: &SandboxSpec,
    net_only: bool,
    program: &Path,
    args: &[OsString],
) -> Result<Vec<OsString>, SandboxError> {
    let mut out = vec![
        OsString::from("--spec"),
        serde_json::to_string(spec)?.into(),
    ];
    if net_only {
        out.push("--net-only".into());
    }
    out.push("--".into());
    out.push(program.as_os_str().to_owned());
    out.extend(args.iter().cloned());
    Ok(out)
}

/// Call first thing in `main`: when the process was started as the helper
/// (`<exe> sandbox-exec …`), confine it and `execve` the agent; otherwise
/// return.
pub fn run_helper_if_requested() {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(OsStr::new(HELPER_SUBCOMMAND)) {
        helper_main(args.collect());
    }
}

/// The helper's argument parsing (everything after the subcommand).
pub fn helper_main(args: Vec<OsString>) -> ! {
    match parse_helper_args(args) {
        Ok(parsed) => confine_and_exec(
            &parsed.spec,
            !parsed.net_only,
            &parsed.program,
            &parsed.args,
        ),
        Err(message) => {
            eprintln!("zeron sandbox-exec: {message}");
            std::process::exit(2);
        }
    }
}

#[derive(Debug, PartialEq)]
struct HelperArgs {
    spec: SandboxSpec,
    net_only: bool,
    program: PathBuf,
    args: Vec<OsString>,
}

fn parse_helper_args(args: Vec<OsString>) -> Result<HelperArgs, String> {
    let mut it = args.into_iter();
    let mut spec = None;
    let mut net_only = false;
    loop {
        let Some(arg) = it.next() else {
            return Err("missing `-- <program>`".into());
        };
        match arg.to_str() {
            Some("--spec") => {
                let json = it.next().ok_or("--spec needs a value")?;
                let json = json.to_str().ok_or("--spec is not UTF-8")?;
                spec = Some(
                    serde_json::from_str::<SandboxSpec>(json)
                        .map_err(|e| format!("bad --spec: {e}"))?,
                );
            }
            Some("--net-only") => net_only = true,
            Some("--") => break,
            _ => return Err(format!("unexpected argument {arg:?}")),
        }
    }
    let spec = spec.ok_or("missing --spec")?;
    let program = PathBuf::from(it.next().ok_or("missing program after --")?);
    Ok(HelperArgs {
        spec,
        net_only,
        program,
        args: it.collect(),
    })
}

/// Confines the current process by `spec` (Landlock + seccomp) and `execve`s
/// `program`. Never returns: on failure it prints why and exits 126 (could
/// not confine — fail closed) or 127 (could not exec).
pub fn exec_confined(spec: &SandboxSpec, program: &Path, args: &[OsString]) -> ! {
    confine_and_exec(spec, true, program, args)
}

fn confine_and_exec(spec: &SandboxSpec, filesystem: bool, program: &Path, args: &[OsString]) -> ! {
    #[cfg(target_os = "linux")]
    {
        let resolved = match Resolved::new(spec, system_temp_dirs()) {
            Ok(resolved) => resolved,
            Err(e) => {
                eprintln!("zeron sandbox-exec: {e}");
                std::process::exit(126);
            }
        };
        if spec.mode != SandboxMode::Off
            && let Err(e) = landlock::confine_current_thread(&resolved, filesystem)
        {
            eprintln!("zeron sandbox-exec: could not confine the agent: {e}");
            std::process::exit(126);
        }
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(program).args(args).exec();
        eprintln!(
            "zeron sandbox-exec: couldn't run {}: {err}",
            program.display()
        );
        std::process::exit(127);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (spec, filesystem, program, args);
        eprintln!("zeron sandbox-exec: the Landlock helper only runs on Linux");
        std::process::exit(126);
    }
}

/// A spec with every path absolute, canonical (macOS `/var` → `/private/var`,
/// symlinks resolved) and de-duplicated, plus the temp folders. Every backend
/// works from this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub mode: SandboxMode,
    pub network: bool,
    pub workspace: PathBuf,
    /// Writable roots: the workspace (WorkspaceWrite), temp folders and
    /// `writable`, minus any nested in another root.
    pub write: Vec<PathBuf>,
    pub write_prefixes: Vec<PathBuf>,
    /// `read_only` entries that sit inside a writable root or prefix (the
    /// others are read-only anyway).
    pub read_only: Vec<PathBuf>,
    /// `hidden`, minus any nested in another hidden path.
    pub hidden: Vec<PathBuf>,
    pub loopback_ports: Vec<u16>,
}

impl Resolved {
    pub(crate) fn new(spec: &SandboxSpec, temp: Vec<PathBuf>) -> Result<Self, SandboxError> {
        let canon = |p: &Path| -> Result<PathBuf, SandboxError> {
            if !p.is_absolute() {
                return Err(SandboxError::RelativePath(p.to_path_buf()));
            }
            Ok(canonicalize_lossy(p))
        };
        let workspace = canon(&spec.workspace)?;
        let mut write = Vec::new();
        if spec.mode == SandboxMode::WorkspaceWrite {
            write.push(workspace.clone());
        }
        for p in temp.iter().chain(&spec.writable) {
            write.push(canon(p)?);
        }
        let write = outermost(write);
        let mut write_prefixes = spec
            .writable_prefixes
            .iter()
            .map(|p| canon(p))
            .collect::<Result<Vec<_>, _>>()?;
        write_prefixes.sort();
        write_prefixes.dedup();
        write_prefixes.retain(|p| !write.iter().any(|w| p.starts_with(w)));
        let mut read_only = spec
            .read_only
            .iter()
            .map(|p| canon(p))
            .collect::<Result<Vec<_>, _>>()?;
        read_only.retain(|r| {
            write.iter().any(|w| r.starts_with(w))
                || write_prefixes
                    .iter()
                    .any(|p| r == p || has_name_prefix(r, p))
        });
        let read_only = outermost(read_only);
        let hidden = outermost(
            spec.hidden
                .iter()
                .map(|p| canon(p))
                .collect::<Result<Vec<_>, _>>()?,
        );
        let mut loopback_ports = spec.loopback_ports.clone();
        loopback_ports.sort_unstable();
        loopback_ports.dedup();
        Ok(Self {
            mode: spec.mode,
            network: spec.network,
            workspace,
            write,
            write_prefixes,
            read_only,
            hidden,
            loopback_ports,
        })
    }

    /// Landlock and bubblewrap can only grant paths that exist.
    fn create_missing_writable_dirs(&self) {
        for dir in &self.write {
            if !dir.exists() && !self.hidden.iter().any(|h| dir.starts_with(h)) {
                let _ = std::fs::create_dir_all(dir);
            }
        }
    }

    /// Ancestors of a protected path (read-only or hidden) that sit inside a
    /// writable root, below the root itself: renaming one would carry the
    /// protected path out from under its rule, so they can't be renamed.
    pub(crate) fn protected_ancestors(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for protected in self.read_only.iter().chain(&self.hidden) {
            for root in &self.write {
                if !protected.starts_with(root) || protected == root {
                    continue;
                }
                let mut a = protected.parent();
                while let Some(dir) = a {
                    if dir == root.as_path() || !dir.starts_with(root) {
                        break;
                    }
                    out.push(dir.to_path_buf());
                    a = dir.parent();
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

/// `path`'s file name starts with `prefix`'s, in the same directory.
fn has_name_prefix(path: &Path, prefix: &Path) -> bool {
    path.parent() == prefix.parent()
        && match (path.file_name(), prefix.file_name()) {
            (Some(n), Some(p)) => n.as_encoded_bytes().starts_with(p.as_encoded_bytes()),
            _ => false,
        }
}

/// Sorted, de-duplicated, without paths nested inside another entry.
fn outermost(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort();
    paths.dedup();
    let mut out: Vec<PathBuf> = Vec::new();
    for p in paths {
        if !out.iter().any(|kept| p.starts_with(kept)) {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SandboxSpec {
        SandboxSpec {
            mode: SandboxMode::WorkspaceWrite,
            network: false,
            workspace: "/zeron-test/home/proj".into(),
            writable: vec![
                "/zeron-test/home/.claude".into(),
                "/zeron-test/home/.claude/projects".into(),
            ],
            writable_prefixes: vec!["/zeron-test/home/.claude.json".into()],
            read_only: vec![
                "/zeron-test/home/proj/.git/hooks".into(),
                "/zeron-test/etc".into(),
                "/zeron-test/home/.claude.json.lock".into(),
            ],
            hidden: vec![
                "/zeron-test/home/.ssh".into(),
                "/zeron-test/home/.ssh/id_ed25519".into(),
            ],
            loopback_ports: vec![27654, 27654],
        }
    }

    #[test]
    fn resolution_dedupes_and_drops_nested_and_irrelevant_entries() {
        let r = Resolved::new(&spec(), vec!["/zeron-test/tmp".into()]).unwrap();
        assert_eq!(
            r.write,
            vec![
                PathBuf::from("/zeron-test/home/.claude"),
                "/zeron-test/home/proj".into(),
                "/zeron-test/tmp".into(),
            ]
        );
        assert_eq!(
            r.write_prefixes,
            vec![PathBuf::from("/zeron-test/home/.claude.json")]
        );
        // `/zeron-test/etc` is outside every writable area: already read-only.
        assert_eq!(
            r.read_only,
            vec![
                PathBuf::from("/zeron-test/home/.claude.json.lock"),
                "/zeron-test/home/proj/.git/hooks".into(),
            ]
        );
        assert_eq!(r.hidden, vec![PathBuf::from("/zeron-test/home/.ssh")]);
        assert_eq!(r.loopback_ports, vec![27654]);
        assert_eq!(
            r.protected_ancestors(),
            vec![PathBuf::from("/zeron-test/home/proj/.git")]
        );
    }

    #[test]
    fn read_only_mode_does_not_write_the_workspace() {
        let mut s = spec();
        s.mode = SandboxMode::ReadOnly;
        let r = Resolved::new(&s, vec![]).unwrap();
        assert!(!r.write.iter().any(|w| w.ends_with("proj")));
        // …so its `.git/hooks` carve-out is moot.
        assert!(!r.read_only.iter().any(|p| p.ends_with("hooks")));
    }

    #[test]
    fn relative_paths_are_rejected() {
        let mut s = spec();
        s.writable.push("relative/dir".into());
        assert!(matches!(
            Resolved::new(&s, vec![]),
            Err(SandboxError::RelativePath(_))
        ));
    }

    #[test]
    fn off_passes_the_command_through() {
        let s = SandboxSpec::new(SandboxMode::Off, false, "/w");
        let w = wrap(&s, Path::new("/bin/echo"), &["hi".into()]).unwrap();
        assert_eq!(w.program, PathBuf::from("/bin/echo"));
        assert_eq!(w.args, vec![OsString::from("hi")]);
        assert_eq!(w.enforcement.backend, Backend::None);
        assert!(
            !w.enforcement.network,
            "network off can't hold without a sandbox"
        );
    }

    #[test]
    fn helper_args_round_trip() {
        let s = spec();
        let args =
            helper_args(&s, true, Path::new("/usr/bin/claude"), &["--print".into()]).unwrap();
        let parsed = parse_helper_args(args).unwrap();
        assert_eq!(
            parsed,
            HelperArgs {
                spec: s,
                net_only: true,
                program: "/usr/bin/claude".into(),
                args: vec!["--print".into()],
            }
        );
        assert!(parse_helper_args(vec!["--".into(), "/bin/sh".into()]).is_err());
        assert!(parse_helper_args(vec!["--bogus".into()]).is_err());
    }

    #[test]
    fn spec_json_is_camel_case_and_skips_empty_lists() {
        let s = SandboxSpec::new(SandboxMode::ReadOnly, true, "/w");
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"mode":"readOnly","network":true,"workspace":"/w"}"#
        );
    }

    #[test]
    fn for_agent_combines_policy_agent_paths_and_git() {
        let policy = AgentPolicy {
            sandbox: SandboxMode::WorkspaceWrite,
            network: false,
            ..AgentPolicy::default()
        };
        let s = SandboxSpec::for_agent(
            HarnessId::Codex,
            &policy,
            "/zeron-test/home/proj",
            Path::new("/zeron-test/home"),
        )
        .with_loopback_port(27654);
        assert_eq!(s.mode, SandboxMode::WorkspaceWrite);
        assert!(!s.network);
        assert!(
            s.writable
                .contains(&PathBuf::from("/zeron-test/home/.codex"))
        );
        assert!(s.hidden.contains(&PathBuf::from("/zeron-test/home/.ssh")));
        assert!(
            s.read_only
                .contains(&PathBuf::from("/zeron-test/home/.codex/packages"))
        );
        // Not a repository: no git carve-outs.
        assert!(!s.read_only.iter().any(|p| p.ends_with("hooks")));
        assert_eq!(s.loopback_ports, vec![27654]);
    }
}
