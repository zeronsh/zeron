//! Isolated trial directories and the engine environment.
//!
//! [`engine_env`] starts from an empty environment (the `env -i` allowlist in
//! the bench plan) and never reads the parent process environment. That is
//! what keeps a bench started from a live agent — where `ZERON_CHAT_ID` and
//! `ZERON_IPC_TOKEN` point at the user's engine — from handing those values
//! to the engine under test.
//!
//! [`assert_isolated`] is the hard stop. [`crate::launch`] calls it after
//! [`Trial::prepare`] and again checks the port immediately before spawn.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

/// The production engine's default IPC port. A trial must never use it.
pub const DEFAULT_IPC_PORT: u16 = 27654;

/// Overrides [`default_zb_root`] when set on the *bench* process.
/// It is not copied into the engine environment.
pub const ZB_ROOT_ENV: &str = "ZB_ROOT";

/// `~/Documents/dev/tmp/zeron-perf`, or `$ZB_ROOT` when that is set.
///
/// The home directory comes from the passwd entry, not from `$HOME`, so a
/// caller that has moved `HOME` still cannot aim the default root at the
/// live profile.
pub fn default_zb_root() -> PathBuf {
    if let Some(root) = std::env::var_os(ZB_ROOT_ENV).filter(|v| !v.is_empty()) {
        return PathBuf::from(root);
    }
    passwd_home()
        .unwrap_or_else(|| PathBuf::from("/home/user"))
        .join("Documents/dev/tmp/zeron-perf")
}

/// One trial directory and the paths the engine is allowed to use.
///
/// `launch` reads [`Self::extra`] as the `extra` argument of [`engine_env`].
/// Put scenario knobs there before [`assert_isolated`] or [`crate::launch`];
/// a direct [`engine_env`] call can preview a different set without storing it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trial {
    /// Bench root. Every isolated path must canonicalize strictly inside it.
    pub zb_root: PathBuf,
    /// `runs/<run>/trials/<index>-<side>/`.
    pub root: PathBuf,
    /// `ZERON_DATA_DIR`. `<root>/data`.
    pub data: PathBuf,
    /// `HOME`. `<root>/home`.
    pub home: PathBuf,
    /// `ZERON_WORKTREES_DIR`. `<root>/worktrees`.
    pub worktrees: PathBuf,
    /// `XDG_RUNTIME_DIR`. `<root>/xdg-runtime`, mode `0700`.
    pub xdg_runtime: PathBuf,
    /// First `PATH` entry. `<root>/shims`.
    pub shims: PathBuf,
    /// `ZB_PROBE_SOCK`. The socket is created by the probe unit, not here.
    pub probe_sock: PathBuf,
    /// Path embedded in `DBUS_SESSION_BUS_ADDRESS`. Never created, so the
    /// session bus connect fails before it can reach the desktop.
    pub dbus_socket: PathBuf,
    /// `ZERON_IPC_PORT`. Never [`DEFAULT_IPC_PORT`] and never a port the
    /// live engine is listening on.
    pub port: u16,
    /// systemd scope unit name without the `.scope` suffix: `zb-<run>-<index>`.
    pub scope_unit: String,
    /// systemd `MemoryMax` (`8G`, `256M`, …).
    pub memory_max: String,
    /// Inserted into `PATH` immediately after [`Self::shims`]. S-mcp only.
    pub node_dir: Option<PathBuf>,
    /// `CLAUDE_CODE_EXECUTABLE`. May live outside `zb_root` (the bench build
    /// dir) but must not sit in `/tmp` or the live profile.
    pub replay_bin: PathBuf,
    /// `zeron-bench` binary invoked as `exec-counted` inside the scope.
    pub bench_bin: PathBuf,
    /// How long [`crate::launch`] waits for the port and for `EngineReady`.
    pub ready_timeout: Duration,
    /// Extra environment pairs appended after the allowlist (`ZB_REPLAY_*`,
    /// `ZERON_SESSION_IDLE_MS`, …). Forbidden keys fail [`assert_isolated`].
    pub extra: Vec<(OsString, OsString)>,
}

impl Trial {
    /// A new trial under `zb_root/runs/<utc>-<label>-<pid>/trials/0-a`.
    ///
    /// Does not create directories and does not pick a port. Call
    /// [`Self::allocate_port`] and [`Self::prepare`] (or [`crate::launch`],
    /// which prepares) before use.
    pub fn create(zb_root: impl Into<PathBuf>, label: &str) -> Result<Self> {
        let stamp = utc_compact(now_unix());
        let run = format!("{stamp}-{}-{}", sanitize(label), std::process::id());
        Self::at(zb_root, &run, 0, "a")
    }

    /// A trial at `zb_root/runs/<run_id>/trials/<index>-<side>`.
    pub fn at(zb_root: impl Into<PathBuf>, run_id: &str, index: u32, side: &str) -> Result<Self> {
        let zb_root = zb_root.into();
        if !zb_root.is_absolute() {
            bail!("ZB_ROOT {} is not absolute", zb_root.display());
        }
        if sanitize(side).is_empty() {
            bail!("trial side is empty");
        }
        let run = sanitize(run_id);
        let root = zb_root
            .join("runs")
            .join(&run)
            .join("trials")
            .join(format!("{index}-{side}"));
        let home = root.join("home");
        Ok(Self {
            data: root.join("data"),
            worktrees: root.join("worktrees"),
            xdg_runtime: root.join("xdg-runtime"),
            shims: root.join("shims"),
            probe_sock: root.join("probe.sock"),
            dbus_socket: root.join("no-bus"),
            home,
            root,
            zb_root,
            port: 0,
            scope_unit: format!("zb-{run}-{index}"),
            memory_max: "8G".into(),
            node_dir: None,
            replay_bin: default_replay_bin(),
            bench_bin: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("zeron-bench")),
            ready_timeout: Duration::from_secs(90),
            extra: Vec::new(),
        })
    }

    /// Bind `127.0.0.1:0`, keep a port that is not forbidden, and release it.
    ///
    /// There is a race between the release and the engine's bind.
    /// [`assert_isolated`] binds the chosen port again and refuses it when
    /// that fails.
    pub fn allocate_port(&mut self) -> Result<()> {
        let forbidden = forbidden_ports();
        for _ in 0..64 {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
                .context("bind 127.0.0.1:0 to pick a trial port")?;
            let port = listener
                .local_addr()
                .context("read the ephemeral trial port")?
                .port();
            drop(listener);
            if port == 0 || forbidden.contains(&port) {
                continue;
            }
            // Confirm the specific port can be taken, then release it.
            match std::net::TcpListener::bind(("127.0.0.1", port)) {
                Ok(held) => {
                    drop(held);
                    self.port = port;
                    return Ok(());
                }
                Err(_) => continue,
            }
        }
        bail!("could not allocate a port that is free and not the live engine's");
    }

    /// Create the trial directories and the bench git identity.
    ///
    /// Refuses a path that is lexically outside [`Self::zb_root`], a symlink
    /// along the way, anything under `/tmp`, and any `.zeron` path component.
    /// Idempotent for an already-prepared trial.
    pub fn prepare(&self) -> Result<()> {
        self.lexical_paths()?;
        let zb = fs::canonicalize(&self.zb_root)
            .with_context(|| format!("ZB_ROOT {} does not exist", self.zb_root.display()))?;
        ensure_not_live_or_tmp(&zb)?;
        for dir in [
            &self.root,
            &self.data,
            &self.home,
            &self.worktrees,
            &self.shims,
            &self.xdg_runtime,
            &self.home.join(".config"),
            &self.home.join(".local/share"),
            &self.home.join(".local/state"),
            &self.home.join(".cache"),
            &self.home.join(".claude"),
        ] {
            create_dir_chain(&zb, &self.zb_root, dir)?;
        }
        for dir in [
            &self.data,
            &self.home,
            &self.xdg_runtime,
            &self.home.join(".claude"),
        ] {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        let gitconfig = self.home.join(".gitconfig");
        fs::write(
            &gitconfig,
            "[user]\n\tname = zeron-bench\n\temail = bench@example.invalid\n",
        )
        .with_context(|| format!("write {}", gitconfig.display()))?;
        fs::set_permissions(&gitconfig, fs::Permissions::from_mode(0o644))?;
        Ok(())
    }

    /// Paths that do not need the directories to exist yet.
    fn lexical_paths(&self) -> Result<()> {
        if !valid_unit(&self.scope_unit) {
            bail!("scope unit {} is not a zb- unit name", self.scope_unit);
        }
        let paths = self.isolated_paths();
        for path in &paths {
            if !path.is_absolute() {
                bail!("{} is not absolute", path.display());
            }
            ensure_not_live_or_tmp(path)?;
            if !lexically_inside(path, &self.zb_root) {
                bail!(
                    "{} is not under ZB_ROOT {}",
                    path.display(),
                    self.zb_root.display()
                );
            }
        }
        if let Some(node) = &self.node_dir {
            ensure_not_live_or_tmp(node)?;
        }
        ensure_not_live_or_tmp(&self.replay_bin)?;
        Ok(())
    }

    fn isolated_paths(&self) -> Vec<PathBuf> {
        vec![
            self.root.clone(),
            self.data.clone(),
            self.home.clone(),
            self.worktrees.clone(),
            self.xdg_runtime.clone(),
            self.shims.clone(),
            self.probe_sock.clone(),
            self.dbus_socket.clone(),
        ]
    }
}

/// `CLOCK_MONOTONIC` nanoseconds. The same clock the probe socket uses, so
/// `t_spawn` / `t_listen` / `t_ready` subtract cleanly from later samples.
pub fn mono_now() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid timespec we own. clock_gettime only writes it.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    if rc != 0 {
        return 0;
    }
    (ts.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(ts.tv_nsec as u64)
}

/// Allowlist environment for the engine. The parent process environment is
/// not consulted: `USER` comes from the passwd entry, and every other value
/// comes from `trial` or `extra`.
///
/// `extra` overrides an allowlist key when the names match (later wins) and
/// is appended otherwise. [`assert_isolated`] rejects the forbidden names
/// and any override that points a path at `/tmp` or the live profile.
pub fn engine_env<K, V>(trial: &Trial, extra: &[(K, V)]) -> Vec<(OsString, OsString)>
where
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let home = &trial.home;
    let mut path = OsString::from(format!("{}:/usr/local/bin:/usr/bin", trial.shims.display()));
    if let Some(node) = &trial.node_dir {
        path = OsString::from(format!(
            "{}:{}:/usr/local/bin:/usr/bin",
            trial.shims.display(),
            node.display()
        ));
    }
    let dbus = OsString::from(format!("unix:path={}", trial.dbus_socket.display()));
    let mut env = Vec::with_capacity(24 + extra.len());
    let user = login_name();
    let port = trial.port.to_string();
    let owned: Vec<(&str, OsString)> = vec![
        ("HOME", home.as_os_str().to_os_string()),
        ("USER", OsString::from(user)),
        ("LANG", OsString::from("C.UTF-8")),
        ("TERM", OsString::from("dumb")),
        ("PATH", path),
        ("XDG_CONFIG_HOME", home.join(".config").into_os_string()),
        ("XDG_DATA_HOME", home.join(".local/share").into_os_string()),
        ("XDG_CACHE_HOME", home.join(".cache").into_os_string()),
        ("XDG_STATE_HOME", home.join(".local/state").into_os_string()),
        (
            "XDG_RUNTIME_DIR",
            trial.xdg_runtime.as_os_str().to_os_string(),
        ),
        ("DBUS_SESSION_BUS_ADDRESS", dbus),
        (
            "GIT_CONFIG_GLOBAL",
            home.join(".gitconfig").into_os_string(),
        ),
        ("GIT_CONFIG_NOSYSTEM", OsString::from("1")),
        ("GIT_TERMINAL_PROMPT", OsString::from("0")),
        ("ZERON_DATA_DIR", trial.data.as_os_str().to_os_string()),
        ("ZERON_IPC_PORT", OsString::from(port)),
        (
            "ZERON_WORKTREES_DIR",
            trial.worktrees.as_os_str().to_os_string(),
        ),
        ("ZERON_EDGE_URL", OsString::from("http://127.0.0.1:9")),
        ("ZERON_WORKOS_CLIENT_ID", OsString::from("")),
        ("ZERON_NO_LOGIN_SHELL", OsString::from("1")),
        (
            "CLAUDE_CODE_EXECUTABLE",
            trial.replay_bin.as_os_str().to_os_string(),
        ),
        ("CLAUDE_CONFIG_DIR", home.join(".claude").into_os_string()),
        ("ZB_PROBE_SOCK", trial.probe_sock.as_os_str().to_os_string()),
    ];
    for (key, value) in owned {
        upsert(&mut env, OsStr::new(key), value.as_os_str());
    }
    for (key, value) in extra {
        upsert(&mut env, key.as_ref(), value.as_ref());
    }
    env
}

/// Refuse to launch unless every guard in the bench plan holds.
///
/// The trial directories must already exist ([`Trial::prepare`]). The check
/// only reads, except for a `bind(127.0.0.1:port)` that it closes again to
/// prove the port is free. It does not open `~/.zeron` for write and it does
/// not flock `engine.lock`.
pub fn assert_isolated(trial: &Trial) -> Result<()> {
    if trial.port == 0 || trial.port == DEFAULT_IPC_PORT {
        bail!(
            "refusing port {}: it is the live engine's default",
            trial.port
        );
    }
    let forbidden = forbidden_ports();
    if forbidden.contains(&trial.port) {
        bail!(
            "refusing port {}: the live engine is listening there",
            trial.port
        );
    }
    trial.lexical_paths()?;
    let zb = fs::canonicalize(&trial.zb_root)
        .with_context(|| format!("ZB_ROOT {} does not exist", trial.zb_root.display()))?;
    ensure_not_live_or_tmp(&zb)?;
    for path in trial.isolated_paths() {
        // probe.sock and no-bus are not created on purpose.
        if path == trial.probe_sock || path == trial.dbus_socket {
            if path.exists() {
                let canon = fs::canonicalize(&path).with_context(|| path.display().to_string())?;
                ensure_inside(&zb, &canon)?;
            }
            continue;
        }
        let canon = fs::canonicalize(&path).with_context(|| {
            format!(
                "{} does not exist (prepare the trial first)",
                path.display()
            )
        })?;
        ensure_inside(&zb, &canon)?;
        ensure_not_live_or_tmp(&canon)?;
    }
    let env = engine_env(trial, &trial.extra);
    check_env(&env, trial, &zb)?;
    reject_credentials(trial, &zb)?;
    // Last, so a bad path never gets as far as touching a socket we might
    // have confused with the live engine. The listener is dropped here.
    std::net::TcpListener::bind(("127.0.0.1", trial.port)).with_context(|| {
        format!(
            "port {} is not free on 127.0.0.1 (bind failed just before launch)",
            trial.port
        )
    })?;
    Ok(())
}

/// Token published by the trial's own engine at `{data_dir}/ipc-token`.
///
/// Does not read `ZERON_IPC_TOKEN` or `ZERON_IPC_TOKEN_FILE` from this
/// process. Those name the user's live engine when the bench is started
/// from an agent. [`zeron_rpc::ipc_auth::client_tokens`] would try them
/// first; the launcher must not.
pub fn trial_ipc_token(data_dir: &Path) -> std::io::Result<Option<String>> {
    zeron_rpc::ipc_auth::read_token(&zeron_rpc::ipc_auth::token_path(data_dir))
}

/// The user's running engine, if `~/.zeron/engine.lock` names a live pid
/// whose command line contains `zeron`.
///
/// Read-only: the lock file is never opened for write and never flocked.
/// A stale pid that now belongs to some other program is ignored so its
/// listen ports are not treated as the engine's. Listen ports come from
/// `/proc/<pid>/net/tcp` rows whose inode is open in that process. The table
/// itself lists every listener in the network namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RealEngine {
    pub pid: u32,
    /// Directory that held `engine.lock` (`~/.zeron`).
    pub data_dir: PathBuf,
    /// TCP ports this process itself is listening on.
    pub ports: Vec<u16>,
}

pub fn detect_real_engine() -> Option<RealEngine> {
    let home = passwd_home()?;
    real_engine_from(&home.join(".zeron").join("engine.lock"), Path::new("/proc"))
}

pub(crate) fn real_engine_from(lock: &Path, proc_root: &Path) -> Option<RealEngine> {
    let raw = fs::read(lock).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let pid: u32 = text.trim().parse().ok()?;
    if pid <= 1 {
        return None;
    }
    let proc = proc_root.join(pid.to_string());
    if !proc.is_dir() {
        return None;
    }
    let cmdline = fs::read(proc.join("cmdline")).unwrap_or_default();
    let comm = fs::read(proc.join("comm")).unwrap_or_default();
    if !bytes_contain(&cmdline, b"zeron") && !bytes_contain(&comm, b"zeron") {
        return None;
    }
    // `/proc/<pid>/net/tcp` is the whole network namespace, not the process.
    // Matching `socket:[inode]` from the process's fd table keeps a trial's
    // own listen port from being treated as the live engine's.
    let owned = socket_inodes(&proc.join("fd"));
    let mut ports = Vec::new();
    if let Some(owned) = owned {
        for name in ["net/tcp", "net/tcp6"] {
            let Ok(text) = fs::read_to_string(proc.join(name)) else {
                continue;
            };
            for (port, inode) in parse_listen_rows(&text) {
                if owned.contains(&inode) {
                    ports.push(port);
                }
            }
        }
    }
    ports.sort_unstable();
    ports.dedup();
    Some(RealEngine {
        pid,
        data_dir: lock.parent().unwrap_or(Path::new("/")).to_path_buf(),
        ports,
    })
}

/// Listen ports (`st == 0A`) from `/proc/net/tcp` or `tcp6` text.
#[cfg(test)]
pub(crate) fn parse_listen_ports(text: &str) -> Vec<u16> {
    parse_listen_rows(text)
        .into_iter()
        .map(|(port, _)| port)
        .collect()
}

/// `(port, inode)` for each `LISTEN` row. The inode matches `socket:[inode]`
/// in `/proc/<pid>/fd`.
fn parse_listen_rows(text: &str) -> Vec<(u16, u64)> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // sl local rem st tx:rx tr:when retrnsmt uid timeout inode
        if fields.len() < 10 || !fields[3].eq_ignore_ascii_case("0A") {
            continue;
        }
        let Some(port_hex) = fields[1].rsplit(':').next() else {
            continue;
        };
        let Ok(port) = u16::from_str_radix(port_hex, 16) else {
            continue;
        };
        let Ok(inode) = fields[9].parse::<u64>() else {
            continue;
        };
        if inode == 0 {
            continue;
        }
        rows.push((port, inode));
    }
    rows
}

/// Socket inodes held by a process, from `/proc/<pid>/fd`. `None` when the
/// directory cannot be read; callers then refuse to guess from the namespace
/// table.
fn socket_inodes(fd_dir: &Path) -> Option<std::collections::HashSet<u64>> {
    let entries = fs::read_dir(fd_dir).ok()?;
    let mut inodes = std::collections::HashSet::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Ok(target) = fs::read_link(entry.path()) else {
            continue;
        };
        let text = target.to_string_lossy();
        let Some(rest) = text.strip_prefix("socket:[") else {
            continue;
        };
        let Some(num) = rest.strip_suffix(']') else {
            continue;
        };
        if let Ok(inode) = num.parse() {
            inodes.insert(inode);
        }
    }
    Some(inodes)
}

fn forbidden_ports() -> Vec<u16> {
    let mut ports = vec![DEFAULT_IPC_PORT];
    #[cfg(test)]
    {
        if let Some(port) = EXTRA_FORBIDDEN_PORTS.with(|cell| cell.get()) {
            ports.push(port);
            ports.sort_unstable();
            ports.dedup();
            return ports;
        }
    }
    if let Some(engine) = detect_real_engine() {
        ports.extend(engine.ports);
    }
    ports.sort_unstable();
    ports.dedup();
    ports
}

#[cfg(test)]
thread_local! {
    static EXTRA_FORBIDDEN_PORTS: std::cell::Cell<Option<u16>> = const { std::cell::Cell::new(None) };
}

fn check_env(env: &[(OsString, OsString)], trial: &Trial, zb: &Path) -> Result<()> {
    if env_key(env, "RUST_LOG").is_some() {
        bail!("RUST_LOG must stay unset so the engine uses production logging");
    }
    for (key, value) in env {
        let key_str = key.to_string_lossy();
        if key_forbidden(&key_str) {
            bail!("engine environment contains forbidden key {key_str}");
        }
        if key.as_bytes().contains(&0) || value.as_bytes().contains(&0) {
            bail!("environment entry {key_str} contains NUL");
        }
        if key_str.contains(['\n', '=']) || value.as_bytes().contains(&b'\n') {
            bail!("environment entry {key_str} contains a newline or '=' in the name");
        }
    }
    let inside = [
        "HOME",
        "ZERON_DATA_DIR",
        "ZERON_WORKTREES_DIR",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
        "XDG_RUNTIME_DIR",
        "GIT_CONFIG_GLOBAL",
        "CLAUDE_CONFIG_DIR",
        "ZB_PROBE_SOCK",
    ];
    for key in inside {
        let value = env_key(env, key).with_context(|| format!("env missing {key}"))?;
        let path = PathBuf::from(value);
        ensure_not_live_or_tmp(&path)?;
        if path.exists() {
            let canon = fs::canonicalize(&path)?;
            ensure_inside(zb, &canon)?;
        } else if !lexically_inside(&path, &trial.zb_root) {
            bail!("{key}={} is not under ZB_ROOT", path.display());
        }
    }
    let dbus =
        env_key(env, "DBUS_SESSION_BUS_ADDRESS").context("env missing DBUS_SESSION_BUS_ADDRESS")?;
    let dbus = dbus.to_string_lossy();
    let Some(dbus_path) = dbus.strip_prefix("unix:path=") else {
        bail!("DBUS_SESSION_BUS_ADDRESS must be unix:path=<trial>/no-bus");
    };
    let dbus_path = PathBuf::from(dbus_path);
    ensure_not_live_or_tmp(&dbus_path)?;
    if !lexically_inside(&dbus_path, &trial.zb_root) {
        bail!("D-Bus socket path is not under ZB_ROOT");
    }
    let replay =
        env_key(env, "CLAUDE_CODE_EXECUTABLE").context("env missing CLAUDE_CODE_EXECUTABLE")?;
    ensure_not_live_or_tmp(&PathBuf::from(replay))?;
    let path = env_key(env, "PATH").context("env missing PATH")?;
    let path = path.to_string_lossy();
    let mut components = path.split(':');
    let first = components.next().unwrap_or("");
    if Path::new(first) != trial.shims {
        bail!("PATH must start with the trial shims dir, got {path}");
    }
    for component in path.split(':') {
        if component.is_empty() {
            continue;
        }
        ensure_not_live_or_tmp(Path::new(component))?;
    }
    let edge = env_key(env, "ZERON_EDGE_URL").context("env missing ZERON_EDGE_URL")?;
    let edge = edge.to_string_lossy();
    if !edge.starts_with("http://127.0.0.1:") {
        bail!("ZERON_EDGE_URL must stay on 127.0.0.1, got {edge}");
    }
    let workos =
        env_key(env, "ZERON_WORKOS_CLIENT_ID").context("env missing ZERON_WORKOS_CLIENT_ID")?;
    if !workos.is_empty() {
        bail!("ZERON_WORKOS_CLIENT_ID must be empty (local-only dev mode)");
    }
    let no_login =
        env_key(env, "ZERON_NO_LOGIN_SHELL").context("env missing ZERON_NO_LOGIN_SHELL")?;
    if no_login != "1" {
        bail!("ZERON_NO_LOGIN_SHELL must be 1");
    }
    Ok(())
}

fn key_forbidden(key: &str) -> bool {
    const EXACT: &[&str] = &[
        "ZERON_CHAT_ID",
        "ZERON_EDGE_TOKEN",
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "RUST_LOG",
    ];
    if EXACT.contains(&key) || key.starts_with("ZERON_IPC_TOKEN") {
        return true;
    }
    let upper = key.to_ascii_uppercase();
    ["TOKEN", "SECRET", "PASSWORD", "CREDENTIAL", "API_KEY"]
        .iter()
        .any(|needle| upper.contains(needle))
}

fn reject_credentials(trial: &Trial, zb: &Path) -> Result<()> {
    let cred = trial.home.join(".claude").join(".credentials.json");
    if path_present(&cred) {
        bail!(
            "trial home contains {} — credentials are never copied",
            cred.display()
        );
    }
    let session = trial.data.join("session.json");
    if path_present(&session) {
        bail!(
            "trial data contains {} — credentials are never copied",
            session.display()
        );
    }
    walk_no_credentials(&trial.root, zb)
}

fn path_present(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn walk_no_credentials(dir: &Path, zb: &Path) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("read {}", dir.display())),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let path = entry.path();
        let file_type = entry.file_type()?;
        if name == ".credentials.json" || name == "agent-accounts" {
            bail!(
                "{} is present in the trial — credentials are never copied",
                path.display()
            );
        }
        if file_type.is_symlink() {
            if let Ok(canon) = fs::canonicalize(&path) {
                ensure_inside(zb, &canon).with_context(|| {
                    format!("{} is a symlink that leaves the bench root", path.display())
                })?;
            }
            // Do not follow it. A link at the live profile must not be walked.
            continue;
        }
        if file_type.is_dir() {
            walk_no_credentials(&path, zb)?;
        }
    }
    Ok(())
}

fn ensure_inside(root: &Path, path: &Path) -> Result<()> {
    if path == root || !path.starts_with(root) {
        bail!("{} is not under {}", path.display(), root.display());
    }
    ensure_not_live_or_tmp(path)
}

fn ensure_not_live_or_tmp(path: &Path) -> Result<()> {
    if is_under_tmp(path) || path_has_dot_zeron(path) {
        bail!("{} is under /tmp or contains /.zeron", path.display());
    }
    if let Ok(canon) = fs::canonicalize(path) {
        if is_under_tmp(&canon) || path_has_dot_zeron(&canon) {
            bail!(
                "{} canonicalizes to {}, which is under /tmp or contains /.zeron",
                path.display(),
                canon.display()
            );
        }
        for live in live_zeron_dirs() {
            if canon == live || canon.starts_with(&live) {
                bail!("{} is the live profile {}", canon.display(), live.display());
            }
        }
    } else {
        for live in live_zeron_dirs() {
            if path == live || path.starts_with(&live) {
                bail!("{} is the live profile {}", path.display(), live.display());
            }
        }
    }
    Ok(())
}

fn is_under_tmp(path: &Path) -> bool {
    let tmp = Path::new("/tmp");
    path == tmp || path.starts_with(tmp)
}

fn path_has_dot_zeron(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == ".zeron")
}

fn lexically_inside(path: &Path, root: &Path) -> bool {
    match path.strip_prefix(root) {
        Ok(rel) => {
            let mut comps = rel.components();
            comps.next().is_some() && rel.components().all(|c| matches!(c, Component::Normal(_)))
        }
        Err(_) => false,
    }
}

fn live_zeron_dirs() -> Vec<PathBuf> {
    let mut homes = Vec::new();
    if let Some(home) = passwd_home() {
        homes.push(home);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if !homes.iter().any(|have| have == &home) {
            homes.push(home);
        }
    }
    homes.into_iter().map(|home| home.join(".zeron")).collect()
}

fn create_dir_chain(zb_canon: &Path, zb_lexical: &Path, target: &Path) -> Result<()> {
    let rel = target.strip_prefix(zb_lexical).with_context(|| {
        format!(
            "{} is not under ZB_ROOT {}",
            target.display(),
            zb_lexical.display()
        )
    })?;
    let mut cur = zb_canon.to_path_buf();
    for comp in rel.components() {
        let Component::Normal(name) = comp else {
            bail!("refusing path component in {}", target.display());
        };
        if name == ".zeron" || name == ".." {
            bail!(
                "refusing component {} in {}",
                name.to_string_lossy(),
                target.display()
            );
        }
        cur.push(name);
        match fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_symlink() => {
                bail!("refusing to use symlink {}", cur.display());
            }
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => bail!("{} exists and is not a directory", cur.display()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&cur).with_context(|| format!("mkdir {}", cur.display()))?;
            }
            Err(err) => return Err(err).with_context(|| format!("stat {}", cur.display())),
        }
    }
    let canon = fs::canonicalize(&cur)?;
    ensure_inside(zb_canon, &canon)?;
    Ok(())
}

fn env_key<'a>(env: &'a [(OsString, OsString)], key: &str) -> Option<&'a OsStr> {
    env.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_os_str())
}

fn upsert(env: &mut Vec<(OsString, OsString)>, key: &OsStr, value: &OsStr) {
    if let Some((_, slot)) = env.iter_mut().find(|(k, _)| k.as_os_str() == key) {
        *slot = value.to_os_string();
    } else {
        env.push((key.to_os_string(), value.to_os_string()));
    }
}

pub(crate) fn env_assignment(key: &OsStr, value: &OsStr) -> OsString {
    let mut pair = key.to_os_string();
    pair.push("=");
    pair.push(value);
    pair
}

pub(crate) fn valid_unit(unit: &str) -> bool {
    let Some(rest) = unit.strip_prefix("zb-") else {
        return false;
    };
    !rest.is_empty()
        && rest.len() <= 200
        && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn bytes_contain(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn login_name() -> String {
    passwd_strings()
        .map(|(name, _)| name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "bench".into())
}

fn passwd_home() -> Option<PathBuf> {
    passwd_strings()
        .map(|(_, home)| home)
        .filter(|home| !home.as_os_str().is_empty())
}

fn passwd_strings() -> Option<(String, PathBuf)> {
    let uid = unsafe { libc::geteuid() };
    let mut buf = vec![0u8; 16 * 1024];
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::zeroed();
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: buf is large enough for a home path; getpwuid_r writes `pwd`
    // and points `result` at it (or null). The C strings are copied before
    // `buf` is dropped.
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            pwd.as_mut_ptr(),
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }
    let pw = unsafe { &*result };
    if pw.pw_name.is_null() || pw.pw_dir.is_null() {
        return None;
    }
    let name = unsafe { std::ffi::CStr::from_ptr(pw.pw_name) };
    let dir = unsafe { std::ffi::CStr::from_ptr(pw.pw_dir) };
    Some((
        name.to_string_lossy().into_owned(),
        PathBuf::from(OsStr::from_bytes(dir.to_bytes())),
    ))
}

fn default_replay_bin() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("zb-replay")))
        .unwrap_or_else(|| PathBuf::from("zb-replay"))
}

fn sanitize(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    if out.is_empty() {
        "run".into()
    } else {
        out.to_string()
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn utc_compact(unix: u64) -> String {
    let secs = unix as i64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400) as u32;
    let (y, m, d) = civil_from_days(days);
    let hh = tod / 3600;
    let mm = (tod % 3600) / 60;
    let ss = tod % 60;
    format!("{y:04}{m:02}{d:02}T{hh:02}{mm:02}{ss:02}Z")
}

/// Howard Hinnant's `civil_from_days`, public domain.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

/// Serializes tests that mutate the process environment.
#[cfg(test)]
pub(crate) fn lock_process_env() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Set a process environment variable from a test.
///
/// `set_var` races with `getenv` on other threads. Callers hold
/// [`lock_process_env`] and only write names the bench tests own, so the
/// tests that mutate the environment do not clobber each other.
#[cfg(test)]
pub(crate) fn set_process_var(key: &str, value: impl AsRef<OsStr>) {
    // SAFETY: see the function docs. The caller holds the test env lock.
    unsafe { std::env::set_var(key, value) }
}

/// Remove a process environment variable from a test. See [`set_process_var`].
#[cfg(test)]
pub(crate) fn unset_process_var(key: &str) {
    // SAFETY: see [`set_process_var`].
    unsafe { std::env::remove_var(key) }
}

#[cfg(test)]
pub(crate) fn test_trial(name: &str) -> Trial {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let home = passwd_home().expect("passwd home");
    let zb = home
        .join("Documents/dev/tmp/zeron-perf/scratch/b0-bench-scaffold/tests")
        .join(format!("{name}-{n}-{}", std::process::id()));
    fs::create_dir_all(&zb).unwrap();
    let mut trial = Trial::at(&zb, &format!("{name}-{n}"), 0, "a").unwrap();
    trial.allocate_port().unwrap();
    trial.prepare().unwrap();
    trial
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_epoch_is_1970() {
        assert_eq!(utc_compact(0), "19700101T000000Z");
    }

    #[test]
    fn engine_env_is_the_allowlist_and_ignores_the_parent() {
        let _guard = lock_process_env();
        let saved: Vec<_> = PARENT_KEYS
            .iter()
            .map(|key| (*key, std::env::var_os(key)))
            .collect();
        struct Restore(Vec<(&'static str, Option<OsString>)>);
        impl Drop for Restore {
            fn drop(&mut self) {
                for (key, value) in self.0.drain(..) {
                    match value {
                        Some(value) => set_process_var(key, value),
                        None => unset_process_var(key),
                    }
                }
            }
        }
        let _restore = Restore(saved);
        for key in PARENT_KEYS {
            set_process_var(key, "parent-secret");
        }
        let trial = test_trial("allowlist");
        let env = engine_env(&trial, &[("ZB_REPLAY_MODE", "mitm")]);
        let keys: Vec<_> = env
            .iter()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            keys,
            vec![
                "HOME",
                "USER",
                "LANG",
                "TERM",
                "PATH",
                "XDG_CONFIG_HOME",
                "XDG_DATA_HOME",
                "XDG_CACHE_HOME",
                "XDG_STATE_HOME",
                "XDG_RUNTIME_DIR",
                "DBUS_SESSION_BUS_ADDRESS",
                "GIT_CONFIG_GLOBAL",
                "GIT_CONFIG_NOSYSTEM",
                "GIT_TERMINAL_PROMPT",
                "ZERON_DATA_DIR",
                "ZERON_IPC_PORT",
                "ZERON_WORKTREES_DIR",
                "ZERON_EDGE_URL",
                "ZERON_WORKOS_CLIENT_ID",
                "ZERON_NO_LOGIN_SHELL",
                "CLAUDE_CODE_EXECUTABLE",
                "CLAUDE_CONFIG_DIR",
                "ZB_PROBE_SOCK",
                "ZB_REPLAY_MODE",
            ]
        );
        for key in PARENT_KEYS {
            assert!(
                env.iter().all(|(k, _)| k != *key),
                "parent {key} leaked into the engine environment"
            );
        }
        assert_eq!(env_key(&env, "HOME").unwrap(), trial.home.as_os_str());
        assert_eq!(
            env_key(&env, "ZERON_DATA_DIR").unwrap(),
            trial.data.as_os_str()
        );
        assert_eq!(
            env_key(&env, "ZERON_WORKTREES_DIR").unwrap(),
            trial.worktrees.as_os_str()
        );
        assert_eq!(
            env_key(&env, "ZERON_IPC_PORT").unwrap(),
            trial.port.to_string().as_str()
        );
        assert_eq!(
            env_key(&env, "ZERON_EDGE_URL").unwrap(),
            "http://127.0.0.1:9"
        );
        assert_eq!(env_key(&env, "ZERON_WORKOS_CLIENT_ID").unwrap(), "");
        assert_eq!(env_key(&env, "ZERON_NO_LOGIN_SHELL").unwrap(), "1");
        assert_eq!(env_key(&env, "LANG").unwrap(), "C.UTF-8");
        assert_eq!(env_key(&env, "TERM").unwrap(), "dumb");
        assert_eq!(
            env_key(&env, "PATH").unwrap(),
            format!("{}:/usr/local/bin:/usr/bin", trial.shims.display()).as_str()
        );
        assert_eq!(
            env_key(&env, "DBUS_SESSION_BUS_ADDRESS").unwrap(),
            format!("unix:path={}", trial.dbus_socket.display()).as_str()
        );
        assert!(env_key(&env, "RUST_LOG").is_none());
        assert!(!trial.home.join(".claude/.credentials.json").exists());
        assert!(!trial.data.join("session.json").exists());
        let gitconfig = fs::read_to_string(trial.home.join(".gitconfig")).unwrap();
        assert!(gitconfig.contains("name = zeron-bench"));
        assert!(gitconfig.contains("email = bench@example.invalid"));
    }

    const PARENT_KEYS: &[&str] = &[
        "ZERON_CHAT_ID",
        "ZERON_IPC_TOKEN",
        "ZERON_IPC_TOKEN_FILE",
        "ZERON_IPC_TOKEN_EXTRA",
        "ZERON_EDGE_TOKEN",
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "RUST_LOG",
        "ANTHROPIC_API_KEY",
    ];

    #[test]
    fn engine_env_inserts_node_dir_after_shims() {
        let mut trial = test_trial("node");
        trial.node_dir = Some(PathBuf::from("/usr/bin"));
        let env = engine_env(&trial, &[] as &[(&str, &str)]);
        let path = env_key(&env, "PATH").unwrap().to_string_lossy();
        assert_eq!(
            path,
            format!("{}:/usr/bin:/usr/local/bin:/usr/bin", trial.shims.display())
        );
    }

    #[test]
    fn assert_isolated_accepts_a_clean_trial() {
        let trial = test_trial("clean");
        assert_isolated(&trial).unwrap();
        assert_ne!(trial.port, DEFAULT_IPC_PORT);
        assert!(trial.port > 0);
    }

    #[test]
    fn assert_isolated_rejects_paths_outside_the_bench_root() {
        let trial = test_trial("outside");
        let outside = trial
            .zb_root
            .parent()
            .unwrap()
            .join(format!("outside-{}", std::process::id()));
        fs::create_dir_all(&outside).unwrap();
        for (label, assign) in [("data", 0u8), ("home", 1), ("worktrees", 2)] {
            let mut bad = trial.clone();
            let path = outside.join(label);
            fs::create_dir_all(&path).unwrap();
            match assign {
                0 => bad.data = path,
                1 => bad.home = path,
                _ => bad.worktrees = path,
            }
            let err = assert_isolated(&bad).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("ZB_ROOT") || msg.contains("not under"),
                "{label}: {msg}"
            );
        }
    }

    #[test]
    fn assert_isolated_rejects_tmp_and_dot_zeron_components() {
        let mut tmp = test_trial("tmp");
        tmp.data = PathBuf::from("/tmp/zb-must-not-exist");
        let err = assert_isolated(&tmp).unwrap_err();
        assert!(err.to_string().contains("/tmp"), "{err}");

        let mut dotted = test_trial("dotted");
        dotted.data = dotted.zb_root.join(".zeron");
        let err = assert_isolated(&dotted).unwrap_err();
        assert!(err.to_string().contains(".zeron"), "{err}");
    }

    #[test]
    fn assert_isolated_rejects_the_live_profile_path() {
        let Some(home) = passwd_home() else { return };
        let live = home.join(".zeron");
        if !live.is_dir() {
            return;
        }
        let mut trial = test_trial("live-path");
        trial.data = live;
        let err = assert_isolated(&trial).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(".zeron") || msg.contains("live profile") || msg.contains("ZB_ROOT"),
            "{msg}"
        );
    }

    #[test]
    fn assert_isolated_rejects_port_27654_without_binding_it() {
        let mut trial = test_trial("default-port");
        trial.port = DEFAULT_IPC_PORT;
        let err = assert_isolated(&trial).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("27654"), "{msg}");
        assert!(msg.contains("refusing"), "{msg}");
    }

    #[test]
    fn assert_isolated_rejects_an_injected_live_port() {
        let mut trial = test_trial("injected-port");
        let port = trial.port;
        // Hold a different free port and declare it to be the live engine's.
        let held = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let live = held.local_addr().unwrap().port();
        assert_ne!(live, port);
        struct Clear;
        impl Drop for Clear {
            fn drop(&mut self) {
                EXTRA_FORBIDDEN_PORTS.with(|cell| cell.set(None));
            }
        }
        let _clear = Clear;
        EXTRA_FORBIDDEN_PORTS.with(|cell| cell.set(Some(live)));
        trial.port = live;
        let err = assert_isolated(&trial).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(&live.to_string()), "{msg}");
        assert!(msg.contains("refusing"), "{msg}");
        drop(held);
    }

    #[test]
    fn assert_isolated_rejects_the_detected_live_port() {
        let Some(engine) = detect_real_engine() else {
            return;
        };
        let Some(&port) = engine.ports.first() else {
            return;
        };
        let mut trial = test_trial("detected-port");
        trial.port = port;
        let err = assert_isolated(&trial).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("refusing"), "{msg}");
        assert!(msg.contains(&port.to_string()), "{msg}");
    }

    #[test]
    fn assert_isolated_rejects_a_port_that_is_already_bound() {
        let mut trial = test_trial("busy-port");
        let held = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        if forbidden_ports().contains(&port) {
            return;
        }
        trial.port = port;
        let err = assert_isolated(&trial).unwrap_err();
        assert!(err.to_string().contains("not free"), "{err}");
        drop(held);
    }

    #[test]
    fn assert_isolated_rejects_each_forbidden_extra_key() {
        let keys = [
            "ZERON_CHAT_ID",
            "ZERON_IPC_TOKEN",
            "ZERON_IPC_TOKEN_FILE",
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "RUST_LOG",
            "ZERON_EDGE_TOKEN",
            "ANTHROPIC_API_KEY",
        ];
        for key in keys {
            let mut trial = test_trial("forbid");
            trial.extra.push((OsString::from(key), OsString::from("x")));
            let err = assert_isolated(&trial).unwrap_err();
            assert!(err.to_string().contains(key), "{key}: {err}");
        }
    }

    #[test]
    fn assert_isolated_rejects_copied_credentials() {
        let cred = test_trial("cred");
        fs::write(cred.home.join(".claude/.credentials.json"), b"{}\n").unwrap();
        assert!(
            assert_isolated(&cred)
                .unwrap_err()
                .to_string()
                .contains("credentials")
        );

        let session = test_trial("session");
        fs::write(session.data.join("session.json"), b"{}\n").unwrap();
        assert!(
            assert_isolated(&session)
                .unwrap_err()
                .to_string()
                .contains("session.json")
        );

        let accounts = test_trial("accounts");
        let dir = accounts.data.join("orgs/dev/agent-accounts");
        fs::create_dir_all(&dir).unwrap();
        assert!(
            assert_isolated(&accounts)
                .unwrap_err()
                .to_string()
                .contains("agent-accounts")
        );
    }

    #[test]
    fn assert_isolated_rejects_a_symlink_that_leaves_the_bench_root() {
        let trial = test_trial("symlink");
        let outside = trial
            .zb_root
            .parent()
            .unwrap()
            .join(format!("linked-creds-{}", std::process::id()));
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join(".credentials.json"), b"not-a-real-secret\n").unwrap();
        let claude = trial.home.join(".claude");
        fs::remove_dir(&claude).unwrap();
        std::os::unix::fs::symlink(&outside, &claude).unwrap();
        let err = assert_isolated(&trial).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("credentials") || msg.contains("symlink") || msg.contains("not under"),
            "{msg}"
        );
    }

    #[test]
    fn parse_listen_ports_reads_hex_listen_rows_only() {
        let text = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:6C06 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1 1 0000000000000000 100 0 0 10 0
   1: 0100007F:1F90 0100007F:0050 01 00000000:00000000 00:00000000 00000000  1000        0 2 1 0000000000000000 100 0 0 10 0
   2: 00000000000000000000000001000000:0050 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000 0 0 3 1 0000000000000000 100 0 0 10 0
";
        assert_eq!(parse_listen_ports(text), vec![27654, 80]);
    }

    #[test]
    fn real_engine_from_ignores_a_reused_pid() {
        let root = test_trial("procfix").zb_root;
        let lock = root.join("engine.lock");
        fs::write(&lock, b"77\n").unwrap();
        let proc = root.join("proc/77");
        fs::create_dir_all(proc.join("net")).unwrap();
        fs::write(proc.join("cmdline"), b"nginx\0").unwrap();
        fs::create_dir_all(proc.join("fd")).unwrap();
        std::os::unix::fs::symlink("socket:[1]", proc.join("fd/3")).unwrap();
        // Inode 1 belongs to this pid. Inode 9 is another listener in the
        // same network namespace and must not count.
        fs::write(
            proc.join("net/tcp"),
            "\
   0: 0100007F:1234 00000000:0000 0A 00000000:00000000 00:00000000 00000000 0 0 1 1 0 100 0 0 10 0
   1: 0100007F:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000 0 0 9 1 0 100 0 0 10 0
",
        )
        .unwrap();
        assert!(real_engine_from(&lock, &root.join("proc")).is_none());

        fs::write(proc.join("cmdline"), b"/usr/bin/zeron-engine\0").unwrap();
        let found = real_engine_from(&lock, &root.join("proc")).unwrap();
        assert_eq!(found.pid, 77);
        assert_eq!(found.ports, vec![0x1234]);
    }

    #[test]
    fn detect_real_engine_does_not_rewrite_the_lock() {
        let Some(home) = passwd_home() else { return };
        let lock = home.join(".zeron/engine.lock");
        let before = fs::read(&lock).ok();
        let found = detect_real_engine();
        let after = fs::read(&lock).ok();
        assert_eq!(before, after);
        if let Some(engine) = found {
            let cmd = fs::read(format!("/proc/{}/cmdline", engine.pid)).unwrap_or_default();
            assert!(bytes_contain(&cmd, b"zeron"));
            assert!(engine.ports.iter().all(|port| *port != 0));
        }
    }

    #[test]
    fn mono_now_moves_forward() {
        let a = mono_now();
        let b = mono_now();
        assert!(a > 0);
        assert!(b >= a);
    }
}
