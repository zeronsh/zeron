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

/// Resolve with execvp's PATH order, excluding every alias of this helper.
/// Keep the chosen path spelling (npx's basename matters to the resolver).
pub fn real_command(command: &OsStr, shim_dir: &Path) -> io::Result<PathBuf> {
    let own = std::env::current_exe()?;
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        if dir == shim_dir {
            continue;
        }
        let path = dir.join(command);
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            path.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        };
        #[cfg(not(unix))]
        let executable = path.is_file();
        if executable && path.canonicalize().is_ok_and(|p| p != own) {
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
        let mut config = crate::Config::parse(
            [
                "--server".into(),
                row.name.clone(),
                "--mode".into(),
                row.mode.clone(),
                "--idle-min".into(),
                launch.idle_minutes.to_string(),
                "--run-dir".into(),
                launch.run_dir.to_str()?.into(),
                "--".into(),
                real.to_str()?.into(),
            ]
            .into_iter()
            .chain(row.args.clone()),
            launch.data_dir,
        )
        .ok()?;
        // H4 cannot name the entry env keys. Hash the effective inherited env,
        // except volatile session markers; persist only the resulting digest.
        config.identity_env = std::env::vars_os()
            .filter_map(|(k, _)| k.into_string().ok())
            .filter(|k| {
                !k.starts_with("ZERON_")
                    && !k.starts_with("CLAUDE_CODE_")
                    && !matches!(
                        k.as_str(),
                        "CLAUDECODE"
                            | "CLAUDE_SESSION_ID"
                            | "CLAUDE_PID"
                            | "PWD"
                            | "OLDPWD"
                            | "SHLVL"
                            | "_"
                    )
            })
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
        Some(config)
    })();
    if let Some(config) = selected {
        // Creating the runtime is the last fallible step before protocol IO.
        if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            let result = runtime.block_on(crate::proxy::run(config));
            runtime.shutdown_timeout(std::time::Duration::from_millis(100));
            return result;
        }
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
