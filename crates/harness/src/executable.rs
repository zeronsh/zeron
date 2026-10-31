//! Cross-platform executable discovery shared by native and ACP harnesses.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Platform {
    Unix,
    Windows,
}

impl Platform {
    pub(crate) fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    home_dir_with(&|key| std::env::var_os(key), Platform::current())
}

/// Resolve a portable user base directory without assuming Unix's `/` exists.
pub(crate) fn home_or_current_dir() -> PathBuf {
    home_or_current_dir_with(
        &|key| std::env::var_os(key),
        &std::env::current_dir,
        Platform::current(),
    )
}

fn home_or_current_dir_with(
    env: &impl Fn(&str) -> Option<OsString>,
    current_dir: &impl Fn() -> std::io::Result<PathBuf>,
    platform: Platform,
) -> PathBuf {
    home_dir_with(env, platform)
        .or_else(|| current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Accept a launchable executable override: native images anywhere, plus the
/// `.cmd`/`.bat` shims npm installs on Windows (the process module launches
/// those through `cmd.exe` with per-argument escaping). The path must exist,
/// so `installed()` and the launch path agree on what an override means.
pub(crate) fn validate_native_override(path: &Path) -> Result<PathBuf, crate::HarnessError> {
    validate_native_override_with(path, Platform::current())
}

fn validate_native_override_with(
    path: &Path,
    platform: Platform,
) -> Result<PathBuf, crate::HarnessError> {
    if platform == Platform::Windows
        && !path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| {
                LAUNCHABLE_WINDOWS_EXTENSIONS
                    .iter()
                    .any(|candidate| extension.eq_ignore_ascii_case(candidate))
            })
    {
        return Err(crate::HarnessError::NotInstalled(format!(
            "{} is not a launchable Windows executable (use .exe, .cmd, .bat, or .com)",
            path.display()
        )));
    }
    if !path.is_file() {
        return Err(crate::HarnessError::NotInstalled(format!(
            "{} does not exist",
            path.display()
        )));
    }
    Ok(path.to_path_buf())
}

pub(crate) fn find_on_paths(exe: &str, extra: Vec<PathBuf>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    find_on_paths_matching_with(
        exe,
        extra,
        &|key| std::env::var_os(key),
        crate::shell_env::user_path(),
        Platform::current(),
        |path| {
            if runnable(path) {
                candidates.push(path.to_path_buf());
            }
            false
        },
    );
    newest_candidate(candidates)
}

pub(crate) fn binary_hint(path: &Path) -> String {
    format!(
        "{} (version {})",
        path.display(),
        binary_version(path)
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unknown".into())
    )
}

/// What a failed agent launch reports. A working directory that is gone
/// fails the spawn with `NotFound` on Unix too, which must not read as a
/// missing binary; `binary` names the binary when it really is missing.
/// Everything else stays [`HarnessError::Io`], kind intact (the engine
/// retries the transient kinds).
pub(crate) fn spawn_error(
    error: std::io::Error,
    cwd: Option<&Path>,
    binary: impl FnOnce() -> String,
) -> crate::HarnessError {
    if let Some(cwd) = cwd
        && matches!(cwd.try_exists(), Ok(false))
    {
        return crate::HarnessError::Io(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            format!("working directory {} does not exist", cwd.display()),
        ));
    }
    if error.kind() == std::io::ErrorKind::NotFound {
        crate::HarnessError::NotInstalled(binary())
    } else {
        crate::HarnessError::Io(error)
    }
}

fn runnable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn newest_candidate(candidates: Vec<PathBuf>) -> Option<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    let candidates: Vec<_> = candidates
        .into_iter()
        .filter(|p| seen.insert(p.canonicalize().unwrap_or_else(|_| p.clone())))
        .collect();
    let mut best = candidates.first()?.clone();
    if candidates.len() > 1 {
        let mut version = binary_version(&best);
        for path in candidates.iter().skip(1) {
            let next = binary_version(path);
            if next.as_ref().is_some_and(|next| {
                version
                    .as_ref()
                    .is_none_or(|current| next.cmp_precedence(current).is_gt())
            }) {
                best = path.clone();
                version = next;
            }
        }
    }
    Some(best)
}

type VersionKey = (PathBuf, Option<std::time::SystemTime>, u64);

struct VersionEntry {
    version: Option<semver::Version>,
    aliases: std::collections::HashSet<String>,
    /// Set for anything but a version: a probe that could not finish
    /// (timeout, transient spawn failure) is retried soon, a definite
    /// failure a little later; only an answer is kept for the binary's life.
    expires: Option<std::time::Instant>,
}

#[derive(Default)]
struct VersionCache {
    entries: std::collections::HashMap<VersionKey, VersionEntry>,
    /// One lock per executable being probed, so concurrent callers for the
    /// same binary share one `--version` while other binaries probe freely.
    probing: std::collections::HashMap<VersionKey, std::sync::Arc<std::sync::Mutex<()>>>,
    /// Per name, how often [`invalidate_versions`] cleared it: a probe whose
    /// names were cleared while it ran may have run the replaced wrapper, so
    /// its answer is not cached.
    invalidated: std::collections::HashMap<String, u64>,
}

impl VersionCache {
    /// A stamp over `names` that changes whenever any of them is invalidated.
    fn invalidation_stamp<'a>(&self, names: impl IntoIterator<Item = &'a String>) -> u64 {
        names
            .into_iter()
            .map(|name| self.invalidated.get(name).copied().unwrap_or(0))
            .sum()
    }
}

static VERSION_CACHE: std::sync::OnceLock<std::sync::Mutex<VersionCache>> =
    std::sync::OnceLock::new();

/// A freshly installed or updated CLI's first exec waits on the OS malware
/// scan, and an npm `.cmd` shim starts node before the CLI itself does.
const VERSION_PROBE_TIMEOUT: std::time::Duration = if cfg!(windows) {
    std::time::Duration::from_secs(10)
} else {
    std::time::Duration::from_secs(5)
};

/// How long an inconclusive probe is trusted before the binary is asked again.
const INCONCLUSIVE_VERSION_TTL: std::time::Duration = std::time::Duration::from_secs(30);
/// How long a definite failure (non-zero exit, no version in the output) is
/// trusted: a busy machine or a half-finished update heals.
const VERSION_FAILURE_TTL: std::time::Duration = std::time::Duration::from_secs(60);

enum Probe {
    /// The binary answered: a version, or a definite failure (non-zero exit,
    /// unparseable output). Cached for the executable's identity.
    Answered(Option<semver::Version>),
    /// The probe could not finish; cached only briefly.
    Inconclusive,
}

fn version_cache() -> std::sync::MutexGuard<'static, VersionCache> {
    VERSION_CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// An explicit install may replace wrappers without changing their metadata.
/// Clear all candidates, including canonical paths behind vendor symlinks.
pub(crate) fn invalidate_versions(names: &[&str]) {
    if VERSION_CACHE.get().is_some() {
        let mut cache = version_cache();
        for name in names {
            *cache.invalidated.entry((*name).to_owned()).or_default() += 1;
        }
        cache
            .entries
            .retain(|_, entry| !names.iter().any(|name| entry.aliases.contains(*name)));
    }
}

/// Probe once per executable identity. A version is cached until the file
/// changes; a failure is retried after [`VERSION_FAILURE_TTL`], and a probe
/// that timed out after [`INCONCLUSIVE_VERSION_TTL`], so one cold first exec
/// cannot pin an older install for the life of the process.
pub fn binary_version(path: &Path) -> Option<semver::Version> {
    binary_version_with(path, VERSION_PROBE_TIMEOUT)
}

fn binary_version_with(path: &Path, timeout: std::time::Duration) -> Option<semver::Version> {
    let canonical = path.canonicalize().ok()?;
    let metadata = canonical.metadata().ok()?;
    let key = (canonical, metadata.modified().ok(), metadata.len());
    let alias = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let cached = |cache: &mut VersionCache| {
        let entry = cache.entries.get_mut(&key)?;
        if entry
            .expires
            .is_some_and(|at| std::time::Instant::now() >= at)
        {
            return None;
        }
        entry.aliases.insert(alias.clone());
        Some(entry.version.clone())
    };
    let slot = {
        let mut cache = version_cache();
        if let Some(version) = cached(&mut cache) {
            return version;
        }
        cache.probing.entry(key.clone()).or_default().clone()
    };
    // The global lock is released while probing: a slow binary must not
    // stall every other harness's resolution behind it.
    let _probing = slot.lock().unwrap_or_else(|e| e.into_inner());
    // The names this binary is known by: what an invalidation would match.
    let (names, stamp) = {
        let mut cache = version_cache();
        if let Some(version) = cached(&mut cache) {
            return version;
        }
        let mut names: Vec<String> = vec![alias.clone()];
        if let Some(entry) = cache.entries.get(&key) {
            names.extend(entry.aliases.iter().cloned());
        }
        let stamp = cache.invalidation_stamp(&names);
        (names, stamp)
    };
    let (version, expires) = match probe_version(path, timeout) {
        Probe::Answered(Some(version)) => (Some(version), None),
        Probe::Answered(None) => (None, Some(std::time::Instant::now() + VERSION_FAILURE_TTL)),
        Probe::Inconclusive => (
            None,
            Some(std::time::Instant::now() + INCONCLUSIVE_VERSION_TTL),
        ),
    };
    let mut cache = version_cache();
    if cache.invalidation_stamp(&names) != stamp {
        // An install replaced wrappers while this probe ran: its answer may
        // be the old one's, so the next caller asks again.
        if cache
            .probing
            .get(&key)
            .is_some_and(|current| std::sync::Arc::ptr_eq(current, &slot))
        {
            cache.probing.remove(&key);
        }
        return version;
    }
    let mut aliases: std::collections::HashSet<String> = [alias].into_iter().collect();
    // An inconclusive retry keeps the aliases earlier callers registered.
    if let Some(previous) = cache.entries.get(&key) {
        aliases.extend(previous.aliases.iter().cloned());
    }
    cache.entries.retain(|(p, _, _), _| p != &key.0);
    cache.entries.insert(
        key.clone(),
        VersionEntry {
            version: version.clone(),
            aliases,
            expires,
        },
    );
    if cache
        .probing
        .get(&key)
        .is_some_and(|current| std::sync::Arc::ptr_eq(current, &slot))
    {
        cache.probing.remove(&key);
    }
    version
}

fn probe_version(path: &Path, timeout: std::time::Duration) -> Probe {
    #[cfg(not(windows))]
    let probe = || -> Probe {
        use std::io::Read;
        use std::process::{Command, Stdio};
        let mut command = Command::new(path);
        command
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let Ok(mut child) = command.spawn() else {
            // ETXTBSY while an updater is still writing the file, EAGAIN, …
            return Probe::Inconclusive;
        };
        fn reader(pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = pipe.take(65536).read_to_end(&mut bytes);
                bytes
            })
        }
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Probe::Inconclusive;
        };
        let readers = [reader(stdout), reader(stderr)];
        let start = std::time::Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if start.elapsed() < timeout => {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        #[cfg(unix)]
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let Some(status) = status else {
            return Probe::Inconclusive;
        };
        if !status.success() {
            return Probe::Answered(None);
        }
        let mut bytes = Vec::new();
        for reader in readers {
            while !reader.is_finished() && start.elapsed() < timeout {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            if !reader.is_finished() {
                return Probe::Inconclusive;
            }
            bytes.extend(reader.join().unwrap_or_default());
            bytes.push(b' ');
        }
        Probe::Answered(parse_version(&bytes))
    };
    #[cfg(windows)]
    let probe = || -> Probe {
        let path = path.to_path_buf();
        // Use the native launcher for npm .cmd/.bat shims and job-tree cleanup.
        // A separate runtime is safe even when descriptors run inside Tokio.
        std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return Probe::Inconclusive;
            };
            runtime.block_on(async {
                let mut command = crate::process::Command::new(&path);
                command
                    .arg("--version")
                    .stdin(crate::process::Stdio::null())
                    .stdout(crate::process::Stdio::piped())
                    .stderr(crate::process::Stdio::piped())
                    .kill_on_drop(true);
                let output = match tokio::time::timeout(timeout, command.output()).await {
                    Ok(Ok(output)) => output,
                    Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                        return Probe::Answered(None);
                    }
                    // Timed out, or a spawn the AV scanner held up.
                    _ => return Probe::Inconclusive,
                };
                if !output.status.success() {
                    return Probe::Answered(None);
                }
                Probe::Answered(
                    parse_version(&output.stdout).or_else(|| parse_version(&output.stderr)),
                )
            })
        })
        .join()
        .unwrap_or(Probe::Inconclusive)
    };
    probe()
}

pub(crate) fn parse_version(bytes: &[u8]) -> Option<semver::Version> {
    String::from_utf8_lossy(bytes)
        .split_whitespace()
        .find_map(|word| {
            semver::Version::parse(
                word.trim_matches(|c: char| matches!(c, '(' | ')' | ','))
                    .trim_start_matches('v'),
            )
            .ok()
        })
}

/// The user's home as the agent CLIs see it. On Windows that is USERPROFILE
/// (Node's `os.homedir()`), not HOME: Git for Windows and MSYS shells export
/// HOME, sometimes as a POSIX path like `/home/ada`, and trusting it would put
/// transcripts, config and installs somewhere the CLIs never look. HOME is
/// only a fallback there, and only when it is an existing absolute path.
fn home_dir_with(env: &impl Fn(&str) -> Option<OsString>, platform: Platform) -> Option<PathBuf> {
    if platform == Platform::Windows {
        return env_path(env, "USERPROFILE").or_else(|| {
            env_path(env, "HOME").filter(|home| is_windows_absolute(home) && home.is_dir())
        });
    }
    env_path(env, "HOME")
}

/// `C:\…` or `\\server\share\…`, judged lexically so it holds on any host.
fn is_windows_absolute(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    drive || text.starts_with(r"\\")
}

fn node_version_manager_bins_with(
    env: &impl Fn(&str) -> Option<OsString>,
    platform: Platform,
) -> Vec<PathBuf> {
    let home = home_dir_with(env, platform);
    let mut dirs = Vec::new();

    if platform == Platform::Windows {
        if let Some(active) = env_path(env, "FNM_MULTISHELL_PATH") {
            dirs.push(active);
        }
    }

    let mut fnm_roots: Vec<PathBuf> = env("FNM_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .into_iter()
        .collect();
    if platform == Platform::Windows {
        if let Some(roaming) = env_path(env, "APPDATA").or_else(|| {
            env_path(env, "USERPROFILE").map(|profile| profile.join("AppData").join("Roaming"))
        }) {
            fnm_roots.push(roaming.join("fnm"));
        }
    }
    if let Some(home) = &home {
        fnm_roots.push(home.join(".local").join("share").join("fnm"));
        fnm_roots.push(home.join("Library").join("Application Support").join("fnm"));
        fnm_roots.push(home.join(".fnm"));
    }
    for root in fnm_roots {
        let default = root.join("aliases").join("default");
        dirs.push(if platform == Platform::Windows {
            default
        } else {
            default.join("bin")
        });
    }

    if platform == Platform::Windows {
        if let Some(dir) = env_path(env, "NVM_SYMLINK") {
            dirs.push(dir);
        }
        if let Some(root) = env_path(env, "VOLTA_HOME").or_else(|| {
            env_path(env, "LOCALAPPDATA")
                .or_else(|| {
                    env_path(env, "USERPROFILE")
                        .map(|profile| profile.join("AppData").join("Local"))
                })
                .map(|local| local.join("Volta"))
        }) {
            dirs.push(root.join("bin"));
        }
        if let Some(dir) = env_path(env, "PNPM_HOME") {
            dirs.push(dir);
        }
        // GUI-launched processes inherit the shell-less PATH: backfill the
        // default global install locations of the common Node ecosystems
        // (npm's global bin, pnpm's default home, scoop, and the per-user
        // ~/.local/bin and ~/.bun/bin used by native installers).
        let local = env_path(env, "LOCALAPPDATA").or_else(|| {
            env_path(env, "USERPROFILE").map(|profile| profile.join("AppData").join("Local"))
        });
        if let Some(npm) = env_path(env, "APPDATA").or_else(|| {
            env_path(env, "USERPROFILE").map(|profile| profile.join("AppData").join("Roaming"))
        }) {
            dirs.push(npm.join("npm"));
        }
        if let Some(local) = &local {
            dirs.push(local.join("pnpm"));
            dirs.push(local.join("Programs").join("nodejs"));
        }
        if let Some(home) = &home {
            dirs.push(home.join("scoop").join("shims"));
            dirs.push(home.join(".local").join("bin"));
            dirs.push(home.join(".bun").join("bin"));
        }
    } else if let Some(home) = &home {
        dirs.push(home.join(".volta").join("bin"));
        dirs.push(home.join(".bun").join("bin"));
        dirs.push(home.join("Library").join("pnpm"));
        dirs.push(home.join(".local").join("share").join("pnpm"));

        let nvm = home.join(".nvm").join("versions").join("node");
        if let Ok(entries) = std::fs::read_dir(&nvm) {
            let mut versions: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path().join("bin"))
                .collect();
            versions.sort();
            versions.reverse();
            dirs.append(&mut versions);
        }
    }
    dirs
}

/// The node-manager and global-install directories discovery searches that
/// exist on this machine, for agent children's PATH: a CLI found through
/// them shells out to `node`/`npx` (MCP servers) from the same place.
#[cfg(windows)]
pub(crate) fn node_manager_dirs() -> Vec<PathBuf> {
    node_version_manager_bins_with(&|key| std::env::var_os(key), Platform::current())
        .into_iter()
        .filter(|dir| dir.is_dir())
        .collect()
}

/// Expand `%NAME%` references the way Windows expands REG_EXPAND_SZ values:
/// names resolve through `lookup`, an unknown reference stays literal, and a
/// lone `%` is kept.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn expand_windows_vars(value: &str, lookup: &impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => {
                let name = &after[..end];
                match lookup(name) {
                    Some(expanded) => out.push_str(&expanded),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn env_path(env: &impl Fn(&str) -> Option<OsString>, key: &str) -> Option<PathBuf> {
    env(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Extensions the spawner can launch on Windows: the PATHEXT subset that maps
/// to a native image (`com`/`exe`) or a script the Windows process module
/// launches through `cmd.exe` (`bat`/`cmd`). Everything else on a user's
/// PATHEXT (`.VBS`, `.MSC`, …) stays undiscoverable — we cannot spawn it.
const LAUNCHABLE_WINDOWS_EXTENSIONS: [&str; 4] = ["com", "exe", "bat", "cmd"];

/// The name variants to probe for `exe` on `platform`, in search order. On
/// Windows the PATHEXT order (defaulting to `.COM;.EXE;.BAT;.CMD`) decides,
/// so a directory's `foo.exe` beats its `foo.cmd` exactly like `cmd.exe`.
fn candidate_names(
    exe: &str,
    env: &impl Fn(&str) -> Option<OsString>,
    platform: Platform,
) -> Vec<OsString> {
    if platform != Platform::Windows || Path::new(exe).extension().is_some() {
        return vec![OsString::from(exe)];
    }
    let configured = env("PATHEXT")
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .to_string_lossy()
                .split(';')
                .filter(|extension| !extension.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let launchable = |extension: &str| {
        LAUNCHABLE_WINDOWS_EXTENSIONS.iter().any(|candidate| {
            extension
                .trim_start_matches('.')
                .eq_ignore_ascii_case(candidate)
        })
    };
    let ordered = if configured.is_empty() {
        LAUNCHABLE_WINDOWS_EXTENSIONS
            .iter()
            .map(|extension| format!(".{extension}"))
            .collect::<Vec<_>>()
    } else {
        configured.into_iter().filter(|e| launchable(e)).collect()
    };
    ordered
        .into_iter()
        .map(|extension| {
            let mut name = OsString::from(exe);
            if !extension.starts_with('.') {
                name.push(".");
            }
            name.push(&extension);
            name
        })
        .collect()
}

/// Extensionless extra locations resolve through the same name variants as
/// PATH directories; an explicit extension is taken as given.
fn extra_variants(path: PathBuf, names: &[OsString], platform: Platform) -> Vec<PathBuf> {
    if platform != Platform::Windows || path.extension().is_some() {
        return vec![path];
    }
    names.iter().map(|name| path.with_file_name(name)).collect()
}

fn is_runnable_candidate(path: &Path, platform: Platform) -> bool {
    if platform == Platform::Windows
        && !path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| {
                LAUNCHABLE_WINDOWS_EXTENSIONS
                    .iter()
                    .any(|candidate| extension.eq_ignore_ascii_case(candidate))
            })
    {
        return false;
    }
    path.is_file()
}

/// Directories of a bare system PATH: launchd's for a Dock/Finder launch,
/// systemd's default for a service, and macOS's path_helper additions.
const SYSTEM_PATH_DIRS: &[&str] = &[
    "/usr/local/sbin",
    "/usr/local/bin",
    "/usr/sbin",
    "/usr/bin",
    "/sbin",
    "/bin",
    "/snap/bin",
    "/usr/games",
    "/usr/local/games",
    "/System/Cryptexes/App/usr/bin",
    "/Library/Apple/usr/bin",
];

/// Whether a Unix process PATH is only the system's: what a GUI launch or a
/// service without its own PATH inherits. Only then does the login shell's
/// PATH (the user's Homebrew, nvm, …) go first; a PATH the user or a unit
/// set up — a venv, nix, direnv, nvm in the launching terminal — keeps
/// precedence, for the child's PATH and for resolving the agent binary alike.
pub(crate) fn is_bare_system_path(path: Option<&OsStr>) -> bool {
    path.is_none_or(|path| {
        std::env::split_paths(path).all(|dir| {
            dir.as_os_str().is_empty()
                || SYSTEM_PATH_DIRS
                    .iter()
                    .any(|system| dir == Path::new(system))
                || dir.starts_with("/var/run/com.apple.security.cryptexd")
        })
    })
}

pub(crate) fn find_on_paths_with(
    exe: &str,
    extra: Vec<PathBuf>,
    env: &impl Fn(&str) -> Option<OsString>,
    login_shell_path: Option<OsString>,
    platform: Platform,
) -> Option<PathBuf> {
    find_on_paths_matching_with(exe, extra, env, login_shell_path, platform, |_| true)
}

pub(crate) fn find_on_paths_matching_with(
    exe: &str,
    extra: Vec<PathBuf>,
    env: &impl Fn(&str) -> Option<OsString>,
    login_shell_path: Option<OsString>,
    platform: Platform,
    mut predicate: impl FnMut(&Path) -> bool,
) -> Option<PathBuf> {
    let names = candidate_names(exe, env, platform);
    let variants =
        |dir: PathBuf| -> Vec<PathBuf> { names.iter().map(|name| dir.join(name)).collect() };
    let from_path = |path: OsString| {
        std::env::split_paths(&path)
            .filter(|dir| !dir.as_os_str().is_empty())
            .flat_map(variants)
            .collect::<Vec<_>>()
    };
    let own_path = env("PATH");
    let login_first = platform == Platform::Unix && is_bare_system_path(own_path.as_deref());
    let own = own_path.map(from_path).unwrap_or_default();
    let shell = login_shell_path.map(from_path).unwrap_or_default();
    // The same precedence as the child's PATH (see `child_path_entries`).
    let mut candidates = if login_first {
        [shell, own].concat()
    } else {
        [own, shell].concat()
    };
    candidates.extend(
        extra
            .into_iter()
            .flat_map(|path| extra_variants(path, &names, platform)),
    );
    candidates.extend(
        node_version_manager_bins_with(env, platform)
            .into_iter()
            .flat_map(variants),
    );
    // npm exposes CLIs as `name.cmd` shims on Windows; those are discoverable
    // and launchable through the PATHEXT variants above, so no per-agent
    // node_modules payload special-casing belongs here.

    candidates
        .into_iter()
        .find(|path| is_runnable_candidate(path, platform) && predicate(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Resolution walks the PATHs in the child PATH's order: the login shell
    /// first only behind a bare system PATH.
    #[cfg(unix)]
    #[test]
    fn resolution_prefers_a_deliberate_process_path_like_the_child_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (venv, brew) = (dir.path().join("venv"), dir.path().join("brew"));
        for bin in [&venv, &brew] {
            std::fs::create_dir_all(bin).unwrap();
            let exe = bin.join("agent");
            std::fs::write(&exe, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let shell = Some(brew.clone().into_os_string());
        let process = std::env::join_paths([venv.as_path(), Path::new("/usr/bin")]).unwrap();
        let lookup = env(&[("PATH", process)]);
        assert_eq!(
            find_on_paths_with("agent", Vec::new(), &lookup, shell.clone(), Platform::Unix),
            Some(venv.join("agent"))
        );
        let bare = env(&[("PATH", OsString::from("/usr/bin:/bin:/usr/sbin:/sbin"))]);
        assert_eq!(
            find_on_paths_with("agent", Vec::new(), &bare, shell, Platform::Unix),
            Some(brew.join("agent"))
        );
        assert!(is_bare_system_path(None));
        assert!(!is_bare_system_path(Some(OsStr::new(
            "/nix/store/x/bin:/usr/bin"
        ))));
    }

    #[test]
    fn a_missing_working_directory_is_not_a_missing_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("deleted-project");
        let not_found = || std::io::Error::from(std::io::ErrorKind::NotFound);
        match spawn_error(not_found(), Some(&gone), || "claude".into()) {
            crate::HarnessError::Io(error) => {
                assert_eq!(error.kind(), std::io::ErrorKind::NotADirectory);
                assert_eq!(
                    error.to_string(),
                    format!("working directory {} does not exist", gone.display())
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            spawn_error(not_found(), Some(tmp.path()), || "claude".into()),
            crate::HarnessError::NotInstalled(hint) if hint == "claude"
        ));
        assert!(matches!(
            spawn_error(not_found(), None, || "claude".into()),
            crate::HarnessError::NotInstalled(_)
        ));
        let busy = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(matches!(
            spawn_error(busy, Some(tmp.path()), || unreachable!()),
            crate::HarnessError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied
        ));
    }

    #[test]
    fn semantic_versions_support_cli_labels_and_prereleases() {
        assert_eq!(
            parse_version(b"Claude Code v2.1.3 (native)"),
            Some(semver::Version::new(2, 1, 3))
        );
        assert!(
            parse_version(b"codex-cli 0.100.0-beta.2").unwrap() < semver::Version::new(0, 100, 0)
        );
        assert!(parse_version(b"unknown").is_none());
    }
    #[cfg(windows)]
    #[test]
    fn npm_shim_versions_are_probed_through_native_launcher() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("old.cmd");
        let second = dir.path().join("new.cmd");
        std::fs::write(&first, "@echo off\r\necho codex-cli 1.0.0\r\n").unwrap();
        std::fs::write(&second, "@echo off\r\necho codex-cli 2.0.0\r\n").unwrap();
        assert_eq!(newest_candidate(vec![first, second.clone()]), Some(second));
    }

    #[cfg(unix)]
    #[test]
    fn explicit_install_invalidates_symlinked_wrapper_versions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let version = dir.path().join("version");
        let entry = dir.path().join("entry.js");
        let cli = dir.path().join("install-cache-test");
        std::fs::write(
            &entry,
            format!("#!/bin/sh\n/bin/cat '{}'\n", version.display()),
        )
        .unwrap();
        std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&entry, &cli).unwrap();
        std::fs::write(&version, "1.0.0").unwrap();
        assert_eq!(binary_version(&cli).unwrap().to_string(), "1.0.0");
        std::fs::write(&version, "2.0.0").unwrap();
        assert_eq!(binary_version(&cli).unwrap().to_string(), "1.0.0");
        invalidate_versions(&["install-cache-test"]);
        assert_eq!(binary_version(&cli).unwrap().to_string(), "2.0.0");
    }

    #[cfg(unix)]
    #[test]
    fn newest_binary_deduplicates_caches_and_invalidates() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("one/codex");
        let second = dir.path().join("two/codex");
        let script = |path: &Path, version: &str| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                path,
                format!(
                    "#!/bin/sh\necho codex-cli {version}\necho x >> '{}.calls'\n",
                    path.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        script(&first, "0.99.0");
        script(&second, "0.110.0");
        let alias = dir.path().join("alias");
        symlink(&second, &alias).unwrap();
        for _ in 0..3 {
            assert_eq!(
                newest_candidate(vec![first.clone(), second.clone(), alias.clone()]),
                Some(second.clone())
            );
        }
        assert_eq!(
            std::fs::read_to_string(second.with_file_name("codex.calls")).unwrap(),
            "x\n"
        );
        script(&first, "1.200.0");
        assert_eq!(
            newest_candidate(vec![first.clone(), second.clone()]),
            Some(first.clone())
        );
        script(&second, "1.200.0");
        assert_eq!(newest_candidate(vec![first.clone(), second]), Some(first));
    }

    #[cfg(unix)]
    #[test]
    fn stderr_versions_are_read_and_build_metadata_does_not_break_ties() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        for (path, version) in [(&first, "1.0.0+aaa"), (&second, "1.0.0+zzz")] {
            std::fs::write(path, format!("#!/bin/sh\necho codex-cli {version} >&2\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(binary_version(&first).unwrap().to_string(), "1.0.0+aaa");
        assert_eq!(newest_candidate(vec![first.clone(), second]), Some(first));
    }

    #[cfg(unix)]
    #[test]
    fn version_probe_bounds_hangs_and_rejects_nonzero() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in [("failed", "echo 9.0.0; exit 1"), ("hung", "sleep 30")] {
            let path = dir.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            let started = std::time::Instant::now();
            assert_eq!(
                binary_version_with(&path, std::time::Duration::from_secs(1)),
                None
            );
            assert!(started.elapsed() < std::time::Duration::from_secs(3));
            assert_eq!(newest_candidate(vec![path.clone()]), Some(path));
        }
    }

    /// A first exec slower than the probe budget (an AV scan of a fresh
    /// install) must not pin "unknown version" for the process lifetime.
    #[cfg(unix)]
    #[test]
    fn timed_out_probe_is_retried_after_its_ttl_but_answers_are_kept() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("slow-until-warm");
        let warm = dir.path().join("warm");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\n[ -e '{}' ] || sleep 30\necho cli 3.1.0\n",
                warm.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let short = std::time::Duration::from_millis(500);
        assert_eq!(binary_version_with(&path, short), None);
        std::fs::write(&warm, "").unwrap();
        // Still inside the TTL: the inconclusive result is reused, not re-run.
        assert_eq!(binary_version_with(&path, short), None);
        // Age the entry past its TTL.
        let canonical = path.canonicalize().unwrap();
        for entry in version_cache()
            .entries
            .iter_mut()
            .filter(|((p, _, _), _)| p == &canonical)
            .map(|(_, entry)| entry)
        {
            assert!(entry.expires.is_some(), "a timeout is cached only briefly");
            entry.expires = Some(std::time::Instant::now());
        }
        assert_eq!(
            binary_version_with(&path, std::time::Duration::from_secs(10)),
            Some(semver::Version::new(3, 1, 0))
        );
        let entries = version_cache();
        let entry = entries
            .entries
            .iter()
            .find(|((p, _, _), _)| p == &canonical)
            .unwrap()
            .1;
        assert!(entry.expires.is_none(), "an answer is kept");
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_probe_is_retried_once_its_failure_ages_out() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let answer = dir.path().join("answer");
        let path = dir.path().join("flaky-cli");
        std::fs::write(
            &path,
            format!("#!/bin/sh\ncat '{}' || exit 1\n", answer.display()),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(binary_version(&path), None);
        // The binary itself is unchanged, so its identity (and cache key) is too.
        std::fs::write(&answer, "flaky-cli 1.2.3\n").unwrap();
        assert_eq!(binary_version(&path), None, "a fresh failure is cached");
        let canonical = path.canonicalize().unwrap();
        for entry in version_cache()
            .entries
            .iter_mut()
            .filter(|((p, _, _), _)| p == &canonical)
            .map(|(_, entry)| entry)
        {
            assert!(entry.expires.is_some(), "a failure is cached only briefly");
            entry.expires = Some(std::time::Instant::now());
        }
        assert_eq!(binary_version(&path), Some(semver::Version::new(1, 2, 3)));
    }

    /// An install that replaces a wrapper in place (same metadata) while it
    /// is being probed: the probe's answer, from the old wrapper, is not
    /// cached over the invalidation.
    #[cfg(unix)]
    #[test]
    fn a_probe_racing_an_invalidation_does_not_cache_its_answer() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("racing-wrapper");
        let (calls, go) = (dir.path().join("calls"), dir.path().join("go"));
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\necho x >> '{}'\nwhile [ ! -e '{}' ]; do sleep 0.02; done\necho cli 1.0.0\n",
                calls.display(),
                go.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let budget = std::time::Duration::from_secs(30);
        let probe = {
            let path = path.clone();
            std::thread::spawn(move || binary_version_with(&path, budget))
        };
        while !calls.exists() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        invalidate_versions(&["racing-wrapper"]);
        std::fs::write(&go, "").unwrap();
        assert_eq!(probe.join().unwrap(), Some(semver::Version::new(1, 0, 0)));
        let canonical = path.canonicalize().unwrap();
        assert!(
            !version_cache()
                .entries
                .keys()
                .any(|(p, _, _)| p == &canonical),
            "the stale answer was cached"
        );
        // The next caller probes again.
        assert_eq!(
            binary_version_with(&path, budget),
            Some(semver::Version::new(1, 0, 0))
        );
        assert_eq!(std::fs::read_to_string(&calls).unwrap(), "x\nx\n");
    }

    /// Concurrent callers for one binary share a probe, and a slow binary
    /// does not hold up probes of another.
    #[cfg(unix)]
    #[test]
    fn concurrent_probes_coalesce_per_binary_without_a_global_wait() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let slow = dir.path().join("slow");
        let fast = dir.path().join("fast");
        std::fs::write(
            &slow,
            format!(
                "#!/bin/sh\necho x >> '{}'\nsleep 3\necho cli 1.0.0\n",
                dir.path().join("slow.calls").display()
            ),
        )
        .unwrap();
        std::fs::write(&fast, "#!/bin/sh\necho cli 2.0.0\n").unwrap();
        for path in [&slow, &fast] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // A budget far past the slow binary's 3s: on a loaded machine the
        // default one could time it out, which is not what this tests.
        let budget = std::time::Duration::from_secs(30);
        let slow_callers: Vec<_> = (0..4)
            .map(|_| {
                let slow = slow.clone();
                std::thread::spawn(move || binary_version_with(&slow, budget))
            })
            .collect();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(
            binary_version_with(&fast, budget),
            Some(semver::Version::new(2, 0, 0))
        );
        assert!(
            slow_callers.iter().all(|caller| !caller.is_finished()),
            "the fast probe waited on the slow one"
        );
        for caller in slow_callers {
            assert_eq!(caller.join().unwrap(), Some(semver::Version::new(1, 0, 0)));
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("slow.calls")).unwrap(),
            "x\n"
        );
    }

    #[test]
    fn windows_home_is_userprofile_not_an_msys_home() {
        let lookup = env(&[
            ("HOME", OsString::from("/home/ada")),
            ("USERPROFILE", OsString::from(r"C:\Users\Ada")),
        ]);
        assert_eq!(
            home_dir_with(&lookup, Platform::Windows),
            Some(PathBuf::from(r"C:\Users\Ada"))
        );
        assert_eq!(
            home_dir_with(&lookup, Platform::Unix),
            Some(PathBuf::from("/home/ada"))
        );
        // Without USERPROFILE, a POSIX-style HOME is not a Windows home.
        let msys_only = env(&[("HOME", OsString::from("/home/ada"))]);
        assert_eq!(home_dir_with(&msys_only, Platform::Windows), None);
        assert!(is_windows_absolute(Path::new(r"D:\Users\ada")));
        assert!(is_windows_absolute(Path::new(r"\\server\share\ada")));
        assert!(!is_windows_absolute(Path::new("/home/ada")));
        assert!(!is_windows_absolute(Path::new(r"Users\ada")));
    }

    #[test]
    fn windows_registry_values_expand_known_variables_only() {
        let lookup = |name: &str| match name {
            "SystemRoot" => Some(r"C:\Windows".to_owned()),
            "NVM_SYMLINK" => Some(r"C:\nvm4w\nodejs".to_owned()),
            _ => None,
        };
        assert_eq!(
            expand_windows_vars(r"%SystemRoot%\system32", &lookup),
            r"C:\Windows\system32"
        );
        assert_eq!(
            expand_windows_vars("%NVM_SYMLINK%", &lookup),
            r"C:\nvm4w\nodejs"
        );
        assert_eq!(
            expand_windows_vars(r"%MISSING%\bin", &lookup),
            r"%MISSING%\bin"
        );
        assert_eq!(expand_windows_vars("100%", &lookup), "100%");
        assert_eq!(expand_windows_vars("a%%b", &lookup), "a%%b");
    }

    fn env(values: &[(&str, OsString)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let values: HashMap<String, OsString> = values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect();
        move |key| values.get(key).cloned()
    }

    fn joined(paths: &[&Path]) -> OsString {
        std::env::join_paths(paths).unwrap()
    }

    #[test]
    fn windows_discovery_uses_pathext_order_and_skips_empty_path_entries() {
        let temp = tempfile::tempdir().unwrap();
        let extensionless = temp.path().join("extensionless");
        let shim = temp.path().join("shim");
        let both = temp.path().join("both");
        let directory = temp.path().join("directory");
        let later = temp.path().join("later");
        std::fs::create_dir_all(&extensionless).unwrap();
        std::fs::create_dir_all(&shim).unwrap();
        std::fs::create_dir_all(&both).unwrap();
        std::fs::create_dir_all(directory.join("agent.exe")).unwrap();
        std::fs::create_dir_all(&later).unwrap();
        // An extensionless file is not a launchable candidate.
        std::fs::write(extensionless.join("agent"), b"shim").unwrap();
        // npm's shim layout: `agent.cmd` alone is discoverable.
        std::fs::write(shim.join("agent.cmd"), b"@echo off").unwrap();
        // Within one directory, PATHEXT order prefers .exe over .cmd.
        std::fs::write(both.join("agent.cmd"), b"@echo off").unwrap();
        std::fs::write(both.join("agent.exe"), b"MZ").unwrap();
        std::fs::write(later.join("agent.exe"), b"MZ").unwrap();

        let path = joined(&[
            Path::new(""),
            &extensionless,
            &shim,
            &both,
            &directory,
            &later,
        ]);
        // Directory order wins across directories: the bare .cmd shim in an
        // earlier directory beats the .exe in a later one, like cmd.exe.
        let found = find_on_paths_with(
            "agent",
            Vec::new(),
            &env(&[("PATH", path.clone())]),
            None,
            Platform::Windows,
        );
        assert_eq!(found, Some(shim.join("agent.cmd")));
        // Once the shim directory is gone, the mixed directory resolves its
        // .exe first, and a directory-shaped candidate is never a match.
        std::fs::remove_dir_all(&shim).unwrap();
        let found = find_on_paths_with(
            "agent",
            Vec::new(),
            &env(&[("PATH", path)]),
            None,
            Platform::Windows,
        );
        assert_eq!(found, Some(both.join("agent.exe")));
    }

    #[test]
    fn windows_pathext_environment_reorders_extensions() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("bins");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("agent.cmd"), b"@echo off").unwrap();
        std::fs::write(dir.join("agent.exe"), b"MZ").unwrap();
        let path = joined(&[&dir]);

        // Default order: .exe before .cmd.
        assert_eq!(
            find_on_paths_with(
                "agent",
                Vec::new(),
                &env(&[("PATH", path.clone())]),
                None,
                Platform::Windows
            ),
            Some(dir.join("agent.exe"))
        );
        // A reordered PATHEXT is honored, but non-launchable entries are ignored.
        // The on-disk name matches PATHEXT's casing: Windows would also match
        // `.CMD`, but this test runs on case-sensitive filesystems too.
        let reordered = env(&[
            ("PATH", path),
            ("PATHEXT", OsString::from(".cmd;.VBS;.EXE")),
        ]);
        let found = find_on_paths_with("agent", Vec::new(), &reordered, None, Platform::Windows)
            .expect("shim variant resolves");
        assert_eq!(found.parent(), Some(dir.as_path()));
        assert!(
            found
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("agent.cmd"))
        );
    }

    #[test]
    fn windows_extra_candidates_resolve_through_pathext_variants() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // Written with the exact casing the extra names below: Windows would
        // also match `tool.cmd`, but this test runs on case-sensitive
        // filesystems too.
        std::fs::write(bin.join("tool.CmD"), b"@echo off").unwrap();
        // Extras are file candidates: an explicit extension is taken as given.
        assert_eq!(
            find_on_paths_with(
                "tool",
                vec![bin.join("tool.CmD")],
                &env(&[]),
                None,
                Platform::Windows,
            ),
            Some(bin.join("tool.CmD")),
        );
        // Extensionless extras (e.g. `~/.local/bin/grok`) resolve through the
        // same name variants as PATH directories — a variant that does not
        // exist on disk is not a match merely by being named.
        assert_eq!(
            find_on_paths_with(
                "grok",
                vec![temp.path().join(".local").join("bin").join("grok")],
                &env(&[]),
                None,
                Platform::Windows,
            ),
            None,
            "the launchable variant must exist on disk, not merely be named"
        );
        let grok_bin = temp.path().join(".local").join("bin");
        std::fs::create_dir_all(&grok_bin).unwrap();
        std::fs::write(grok_bin.join("grok.cmd"), b"@echo off").unwrap();
        assert_eq!(
            find_on_paths_with(
                "grok",
                vec![grok_bin.join("grok")],
                &env(&[]),
                None,
                Platform::Windows,
            ),
            Some(grok_bin.join("grok.cmd")),
            "an extensionless extra resolves its .cmd shim"
        );
        assert_eq!(
            extra_variants(
                temp.path().join("other").join("grok"),
                &candidate_names("grok", &env(&[]), Platform::Windows),
                Platform::Windows,
            ),
            vec![
                temp.path().join("other").join("grok.com"),
                temp.path().join("other").join("grok.exe"),
                temp.path().join("other").join("grok.bat"),
                temp.path().join("other").join("grok.cmd"),
            ],
        );
    }

    #[test]
    fn windows_node_manager_dirs_use_explicit_environment_and_userprofile_fallback() {
        let profile = PathBuf::from(r"C:\Users\Ada");
        let nvm = PathBuf::from(r"D:\Node Current");
        let volta = PathBuf::from(r"E:\Volta");
        let pnpm = PathBuf::from(r"F:\pnpm");
        let lookup = env(&[
            ("USERPROFILE", profile.clone().into_os_string()),
            ("NVM_SYMLINK", nvm.clone().into_os_string()),
            ("VOLTA_HOME", volta.clone().into_os_string()),
            ("PNPM_HOME", pnpm.clone().into_os_string()),
        ]);

        assert_eq!(home_dir_with(&lookup, Platform::Windows), Some(profile));
        let bins = node_version_manager_bins_with(&lookup, Platform::Windows);
        assert!(bins.contains(&nvm));
        assert!(bins.contains(&volta.join("bin")));
        assert!(bins.contains(&pnpm));
    }

    #[test]
    fn windows_node_manager_discovery_finds_active_and_default_installations() {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("Profile with spaces");
        let roaming = profile.join("AppData/Roaming");
        let local = profile.join("AppData/Local");
        let active = temp.path().join("active fnm");
        let explicit = temp.path().join("custom fnm");
        let defaults = [
            active.clone(),
            explicit.join("aliases/default"),
            roaming.join("fnm/aliases/default"),
            local.join("Volta/bin"),
        ];
        for dir in &defaults {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("node.exe"), b"MZ").unwrap();
        }
        let lookup = env(&[
            ("USERPROFILE", profile.into_os_string()),
            ("FNM_MULTISHELL_PATH", active.into_os_string()),
            ("FNM_DIR", explicit.into_os_string()),
        ]);
        // Exercise selection, including fallback after a stale active link.
        for dir in defaults {
            assert_eq!(
                find_on_paths_with("node", vec![], &lookup, None, Platform::Windows),
                Some(dir.join("node.exe"))
            );
            std::fs::remove_file(dir.join("node.exe")).unwrap();
        }
        assert_eq!(
            find_on_paths_with("node", vec![], &lookup, None, Platform::Windows),
            None
        );
    }

    #[test]
    fn windows_node_manager_defaults_honor_redirected_appdata_and_volta_override() {
        let roaming = PathBuf::from("redirected roaming");
        let local = PathBuf::from("redirected local");
        let volta = PathBuf::from("custom volta");
        let lookup = env(&[
            ("APPDATA", roaming.clone().into_os_string()),
            ("LOCALAPPDATA", local.clone().into_os_string()),
            ("VOLTA_HOME", volta.clone().into_os_string()),
        ]);
        let bins = node_version_manager_bins_with(&lookup, Platform::Windows);
        assert!(bins.contains(&roaming.join("fnm/aliases/default")));
        assert!(bins.contains(&volta.join("bin")));
        assert!(!bins.contains(&local.join("Volta/bin")));
        let bins = node_version_manager_bins_with(&lookup, Platform::Unix);
        assert!(!bins.contains(&roaming.join("fnm/aliases/default")));
        assert!(!bins.contains(&volta.join("bin")));
    }

    #[test]
    fn unix_pi_discovery_finds_node_manager_defaults_without_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let lookup = env(&[("HOME", home.as_os_str().to_owned())]);
        for relative in [
            ".volta/bin",
            ".bun/bin",
            ".local/share/pnpm",
            "Library/pnpm",
            ".nvm/versions/node/v22/bin",
            ".fnm/aliases/default/bin",
            ".local/share/fnm/aliases/default/bin",
            ".local/bin",
        ] {
            let bin = home.join(relative);
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(bin.join("pi"), "fixture").unwrap();
            // Pi supplies ~/.local/bin as an explicit npm fallback.
            let extra = vec![home.join(".local/bin/pi")];
            assert_eq!(
                find_on_paths_with("pi", extra, &lookup, None, Platform::Unix),
                Some(bin.join("pi")),
                "{relative}"
            );
            std::fs::remove_file(bin.join("pi")).unwrap();
        }
    }

    #[test]
    fn unix_discovery_preserves_exact_name_and_source_order() {
        let temp = tempfile::tempdir().unwrap();
        let path_dir = temp.path().join("path");
        let shell_dir = temp.path().join("shell");
        let extra_dir = temp.path().join("extra");
        for dir in [&path_dir, &shell_dir, &extra_dir] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("agent"), b"shim").unwrap();
        }
        let found = find_on_paths_with(
            "agent",
            vec![extra_dir.join("agent")],
            &env(&[("PATH", joined(&[&path_dir]))]),
            Some(joined(&[&shell_dir])),
            Platform::Unix,
        );
        assert_eq!(found, Some(path_dir.join("agent")));
    }

    #[test]
    fn windows_native_overrides_accept_batch_shims_and_reject_the_unlaunchable() {
        let temp = tempfile::tempdir().unwrap();
        // Batch shims are valid overrides (launched through cmd.exe), and the
        // extension check is case-insensitive.
        for name in [
            "agent.cmd",
            "agent.CmD",
            "agent.BAT",
            "agent.exe",
            "agent.EXE",
        ] {
            let path = temp.path().join(name);
            std::fs::write(&path, b"MZ").unwrap();
            assert_eq!(
                validate_native_override_with(&path, Platform::Windows).unwrap(),
                path
            );
        }
        // Extensions the spawner cannot launch are rejected…
        let script = temp.path().join("agent.ps1");
        std::fs::write(&script, b"exit 0").unwrap();
        let error = validate_native_override_with(&script, Platform::Windows).unwrap_err();
        assert!(error.to_string().contains("launchable"), "{error}");
        // …and so are overrides that do not exist.
        let missing = temp.path().join("missing.exe");
        let error = validate_native_override_with(&missing, Platform::Windows).unwrap_err();
        assert!(error.to_string().contains("does not exist"), "{error}");
        // Unix keeps extension-free judgment and still checks existence.
        let unix_script = temp.path().join("agent.sh");
        std::fs::write(&unix_script, b"#!/bin/sh\n").unwrap();
        assert_eq!(
            validate_native_override_with(&unix_script, Platform::Unix).unwrap(),
            unix_script
        );
        assert!(validate_native_override_with(&temp.path().join("agent"), Platform::Unix).is_err());
    }

    #[test]
    fn portable_base_dir_uses_current_dir_when_home_is_unset() {
        let current = PathBuf::from(r"D:\work\comet");
        assert_eq!(
            home_or_current_dir_with(&env(&[]), &|| Ok(current.clone()), Platform::Windows,),
            current
        );
    }
}
