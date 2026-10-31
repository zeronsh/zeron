//! PATH multi-call entry. No runtime, config IO, or logging on the shell path.
use serde::{Deserialize, Serialize};
use std::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
};

pub const DIR_ENV: &str = "ZERON_MCP_SHIM_DIR";
pub const PARENT_ENV: &str = "ZERON_MCP_SHIM_PARENT";
pub const KEY_ENV: &str = "ZERON_MCP_SHIM_KEY";
pub const MANIFEST: &str = "launch.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Launch {
    pub version: u32,
    pub key: String,
    pub parent_exe: PathBuf,
    pub parent_identity: String,
    pub data_dir: PathBuf,
    pub run_dir: PathBuf,
    pub idle_minutes: u32,
    pub servers: Vec<Server>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Server {
    pub name: String,
    pub command: String,
    pub resolved_command: PathBuf,
    pub args: Vec<String>,
    pub mode: String,
    pub channel: bool,
    pub owner_legacy: bool,
}

/// Ambiguous command lines stay eager, including a lazy/eager duplicate.
pub fn decide<'a>(
    launch: &'a Launch,
    parent_matches: bool,
    key: &str,
    command: &str,
    resolved: &Path,
    args: &[OsString],
) -> Option<&'a Server> {
    if !parent_matches || launch.version != 1 || launch.key != key {
        return None;
    }
    let mut matches = launch.servers.iter().filter(|s| {
        s.command == command
            && s.resolved_command == resolved
            && s.args.len() == args.len()
            && s.args.iter().zip(args).all(|(a, b)| OsStr::new(a) == b)
    });
    let row = matches.next()?;
    (matches.next().is_none()
        && !row.channel
        && !row.owner_legacy
        && matches!(row.mode.as_str(), "lazy" | "lazy-keep-alive"))
    .then_some(row)
}

/// Whether an inherited variable takes part in the cache identity. The shim
/// cannot tell the entry's own env from the inherited one, so it hashes
/// everything except values that change per Zeron/Claude session, login,
/// terminal or service start and never decide what a server offers. Hashing
/// them would give every such change a fresh discovery and a stale cache file
/// that is never collected. A wrongly excluded value only costs a refreshed
/// catalog: every real connection refreshes the metadata.
pub fn identity_env_key(key: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "ZERON_",
        "CLAUDE_CODE_",
        "SSH_",
        "VSCODE_",
        "__MISE_",
        "KITTY_",
        "ITERM_",
    ];
    const NAMES: &[&str] = &[
        "CLAUDECODE",
        "CLAUDE_SESSION_ID",
        "CLAUDE_PID",
        "PWD",
        "OLDPWD",
        "SHLVL",
        "_",
        "COLUMNS",
        "LINES",
        "INVOCATION_ID",
        "JOURNAL_STREAM",
        "DBUS_SESSION_BUS_ADDRESS",
        "XDG_SESSION_ID",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "WINDOWID",
        "TMUX",
        "TMUX_PANE",
        "STY",
        "TERM_SESSION_ID",
        "SESSION_MANAGER",
        "GPG_AGENT_INFO",
        "HYPRLAND_INSTANCE_SIGNATURE",
        "SWAYSOCK",
        "I3SOCK",
    ];
    !PREFIXES.iter().any(|p| key.starts_with(p)) && !NAMES.contains(&key)
}

/// `path` without any entry equal to `dir`; None when nothing was removed.
/// Every other entry keeps its exact bytes, order and empty slots.
pub fn path_without(path: &OsStr, dir: &Path) -> Option<OsString> {
    let mut removed = false;
    let kept: Vec<_> = std::env::split_paths(path)
        .filter(|p| {
            let hit = p == dir;
            removed |= hit;
            !hit
        })
        .collect();
    removed.then(|| std::env::join_paths(kept).ok()).flatten()
}

/// Resolve with execvp's PATH order, excluding every alias of this helper.
/// Keep the chosen path spelling (npx's basename matters to the resolver).
pub fn real_command(command: &OsStr, shim_dir: &Path) -> io::Result<PathBuf> {
    let own = std::env::current_exe()?;
    #[cfg(unix)]
    let own_meta = own.metadata()?;
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        if dir == shim_dir {
            continue;
        }
        let mut path = dir.join(command);
        if path.parent().is_none_or(|p| p.as_os_str().is_empty()) {
            // An empty PATH slot means the cwd. Keep a slash so exec never
            // falls back to a PATH search that could find this shim again.
            path = Path::new(".").join(command);
        }
        let Ok(meta) = path.metadata() else {
            continue;
        };
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            meta.is_file()
                && meta.permissions().mode() & 0o111 != 0
                && (meta.dev(), meta.ino()) != (own_meta.dev(), own_meta.ino())
        };
        #[cfg(not(unix))]
        let executable = meta.is_file() && path.canonicalize().is_ok_and(|p| p != own);
        if executable {
            return Ok(path);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "shim command not found on PATH",
    ))
}

#[cfg(target_os = "linux")]
fn parent_exe() -> io::Result<PathBuf> {
    // getppid is race checked again after reading the manifest.
    std::fs::read_link(format!("/proc/{}/exe", unsafe { libc::getppid() }))
}
#[cfg(not(target_os = "linux"))]
fn parent_exe() -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "launch interception requires Linux",
    ))
}

/// Linux retains the executed pathname separately from argv0 (exec -a).
/// Use it for dispatch while preserving the caller's literal argv0 on exec.
pub fn invoked_path(arg0: &OsStr) -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let ptr = unsafe { libc::getauxval(libc::AT_EXECFN) } as *const libc::c_char;
        if !ptr.is_null() {
            return PathBuf::from(OsStr::from_bytes(
                unsafe { std::ffi::CStr::from_ptr(ptr) }.to_bytes(),
            ));
        }
    }
    PathBuf::from(arg0)
}

/// The argv0 entry selects this mode even if its manifest/env is missing.
/// All pre-protocol failures exec the real binary; never consume stdin first.
pub fn run(argv: Vec<OsString>) -> io::Result<()> {
    let arg0 = argv
        .first()
        .ok_or_else(|| io::Error::other("missing argv0"))?;
    let invoked = invoked_path(arg0);
    let command = invoked
        .file_name()
        .ok_or_else(|| io::Error::other("missing command"))?;
    let dir = std::env::var_os(DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| invoked.parent().unwrap_or(Path::new("")).to_owned());
    // Programs that locate themselves by searching PATH for their argv0 (Python
    // finding its venv/prefix) must find the real executable, not this entry.
    // Only this process's PATH changes, and only by dropping the shim directory.
    if let Some(path) = std::env::var_os("PATH").and_then(|p| path_without(&p, &dir)) {
        // SAFETY: still the single main thread; no runtime or thread exists yet.
        unsafe { std::env::set_var("PATH", path) };
    }
    let real = real_command(command, &dir)?;
    let selected = (|| -> Option<crate::Config> {
        // Shell children take this branch without even opening launch.json.
        // A configured server has the configured launcher as argv0. Keep
        // explicit exec -a invocations on their ordinary executable path.
        if Path::new(arg0).file_name()? != command {
            return None;
        }
        let parent = parent_exe().ok()?;
        if std::env::var_os(PARENT_ENV).as_deref() != Some(parent.as_os_str()) {
            return None;
        }
        let raw = std::fs::read(dir.join(MANIFEST)).ok()?;
        let launch: Launch = serde_json::from_slice(&raw).ok()?;
        if parent != launch.parent_exe || parent_exe().ok()? != parent {
            return None;
        }
        if crate::cache::binary_id(&parent) != launch.parent_identity {
            return None;
        }
        let key = std::env::var(KEY_ENV).ok()?;
        let row = decide(
            &launch,
            true,
            &key,
            command.to_str()?,
            &real.canonicalize().ok()?,
            &argv[1..],
        )?;
        let mut config =
            crate::Config::parse(proxy_args(&launch, row, &real)?, launch.data_dir.clone()).ok()?;
        // H4 cannot name the entry env keys. Hash the effective inherited env,
        // except volatile session markers; persist only the resulting digest.
        config.identity_env = std::env::vars_os()
            .filter_map(|(k, _)| k.into_string().ok())
            .filter(|k| identity_env_key(k))
            .collect();
        config.identity_path = Some(
            std::env::join_paths(
                std::env::split_paths(&std::env::var_os("PATH")?).filter(|p| p != &dir),
            )
            .ok()?
            .into_string()
            .ok()?,
        );
        config.plugin_root = std::env::var_os("CLAUDE_PLUGIN_ROOT").map(PathBuf::from);
        // JSON cache identities cannot faithfully represent non-UTF8 env.
        for key in &config.identity_env {
            std::env::var(key).ok()?;
        }
        config.child_argv0 = Some(arg0.clone());
        Some(config)
    })();
    if let Some(config) = selected {
        return crate::run_config(config);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(std::process::Command::new(real)
            .arg0(arg0)
            .args(&argv[1..])
            .exec())
    }
    #[cfg(not(unix))]
    {
        let _ = real;
        Err(io::Error::other("shim exec unsupported"))
    }
}

fn proxy_args(launch: &Launch, row: &Server, real: &Path) -> Option<Vec<String>> {
    Some(
        [
            "--server".into(),
            row.name.clone(),
            "--mode".into(),
            row.mode.clone(),
            "--idle-min".into(),
            launch.idle_minutes.to_string(),
            "--run-dir".into(),
            launch.run_dir.to_str()?.into(),
            "--data-dir".into(),
            launch.data_dir.to_str()?.into(),
            "--".into(),
            real.to_str()?.into(),
        ]
        .into_iter()
        .chain(row.args.clone())
        .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn launch() -> Launch {
        Launch {
            version: 1,
            key: "fresh".into(),
            parent_exe: "/claude".into(),
            parent_identity: "id".into(),
            data_dir: "data".into(),
            run_dir: "run".into(),
            idle_minutes: 10,
            servers: vec![Server {
                name: "plugin:p:s".into(),
                command: "node".into(),
                resolved_command: "/usr/bin/node".into(),
                args: vec!["server.js".into()],
                mode: "lazy".into(),
                channel: false,
                owner_legacy: false,
            }],
        }
    }
    #[test]
    fn handoff_passes_explicit_data_dir() {
        let launch = launch();
        let args = proxy_args(&launch, &launch.servers[0], Path::new("/usr/bin/node")).unwrap();
        let flag = args.iter().position(|arg| arg == "--data-dir").unwrap();
        assert_eq!(args[flag + 1], "data");
        let config = crate::Config::parse(args, "/unused-default".into()).unwrap();
        assert_eq!(config.data_dir, launch.data_dir);
    }
    fn chosen(
        l: &Launch,
        parent: bool,
        key: &str,
        command: &str,
        real: &str,
        args: &[&str],
    ) -> bool {
        decide(
            l,
            parent,
            key,
            command,
            Path::new(real),
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
        )
        .is_some()
    }
    #[test]
    fn path_without_drops_only_the_shim_dir_and_keeps_every_other_slot() {
        let dir = Path::new("/run/shim");
        let drop = |p: &str| path_without(OsStr::new(p), dir).map(|v| v.into_string().unwrap());
        assert_eq!(drop("/run/shim:/a::/b"), Some("/a::/b".into()));
        assert_eq!(drop("/a:/run/shim/:/b:/run/shim"), Some("/a:/b".into()));
        assert_eq!(drop("/a::/b"), None);
        assert_eq!(drop(""), None);
        assert_eq!(drop("/run/shim-2:/a"), None);
    }
    #[test]
    fn identity_env_skips_session_volatile_values_but_keeps_server_config() {
        for volatile in [
            "ZERON_MCP_SHIM_KEY",
            "CLAUDE_CODE_SESSION_ID",
            "INVOCATION_ID",
            "SSH_AUTH_SOCK",
            "__MISE_DIFF",
            "TMUX_PANE",
            "COLUMNS",
            "WAYLAND_DISPLAY",
        ] {
            assert!(!identity_env_key(volatile), "{volatile}");
        }
        for config in [
            "HOME",
            "PATH",
            "GITHUB_TOKEN",
            "NODE_OPTIONS",
            "CLAUDE_PLUGIN_ROOT",
            "CLAUDE_PLUGIN_DATA",
            "XDG_CONFIG_HOME",
            "LANG",
            "MISE_SHELL",
        ] {
            assert!(identity_env_key(config), "{config}");
        }
    }
    #[test]
    fn shim_requires_all_three_conditions() {
        let l = launch();
        assert!(chosen(
            &l,
            true,
            "fresh",
            "node",
            "/usr/bin/node",
            &["server.js"]
        ));
        assert!(!chosen(
            &l,
            false,
            "fresh",
            "node",
            "/usr/bin/node",
            &["server.js"]
        ));
        assert!(!chosen(
            &l,
            true,
            "stale",
            "node",
            "/usr/bin/node",
            &["server.js"]
        ));
        assert!(!chosen(
            &l,
            true,
            "fresh",
            "node",
            "/other/node",
            &["server.js"]
        ));
        assert!(!chosen(
            &l,
            true,
            "fresh",
            "node",
            "/usr/bin/node",
            &["other.js"]
        ));
        assert!(!chosen(
            &l,
            true,
            "fresh",
            "node",
            "/usr/bin/node",
            &["server.js", "extra"]
        ));
        assert!(!chosen(
            &l,
            true,
            "fresh",
            "/usr/bin/node",
            "/usr/bin/node",
            &["server.js"]
        ));
    }
    #[test]
    fn shim_never_proxies_channels_owners_eager_off_or_ambiguous_rows() {
        for mode in ["eager", "off", "unknown"] {
            let mut l = launch();
            l.servers[0].mode = mode.into();
            assert!(!chosen(
                &l,
                true,
                "fresh",
                "node",
                "/usr/bin/node",
                &["server.js"]
            ));
        }
        let mut l = launch();
        l.servers[0].channel = true;
        assert!(!chosen(
            &l,
            true,
            "fresh",
            "node",
            "/usr/bin/node",
            &["server.js"]
        ));
        l.servers[0].channel = false;
        l.servers[0].owner_legacy = true;
        assert!(!chosen(
            &l,
            true,
            "fresh",
            "node",
            "/usr/bin/node",
            &["server.js"]
        ));
        l.servers[0].owner_legacy = false;
        l.servers.push(l.servers[0].clone());
        assert!(!chosen(
            &l,
            true,
            "fresh",
            "node",
            "/usr/bin/node",
            &["server.js"]
        ));
        l.servers.pop();
        l.version = 2;
        assert!(!chosen(
            &l,
            true,
            "fresh",
            "node",
            "/usr/bin/node",
            &["server.js"]
        ));
        l.version = 1;
        l.servers[0].mode = "lazy-keep-alive".into();
        assert!(chosen(
            &l,
            true,
            "fresh",
            "node",
            "/usr/bin/node",
            &["server.js"]
        ));
    }
}
