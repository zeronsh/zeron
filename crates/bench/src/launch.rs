//! Start and stop an engine inside a transient user scope.
//!
//! The scope command is `systemd-run --user --scope … -- /usr/bin/env -i
//! … zeron-bench exec-counted -- <engine>`. `systemd-run` keeps the bench's
//! environment so it can reach the user bus; `env -i` is what the engine
//! actually execs, so the allowlist from [`crate::engine_env`] is the whole
//! environment. `exec-counted` counts the engine and writes `perf.json`.
//!
//! If `systemd-run` cannot talk to the user bus, the engine is put in its
//! own process group instead. [`ShutdownReport::containment`] is `"scope"`
//! or `"pgroup"`. Stop asks `StopEngine`, then SIGTERM, then SIGKILL, and
//! writes `cgroup.kill` only for a `zb-*` scope this launch created.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Write;
use std::net::TcpListener;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tokio::net::TcpStream;

use crate::env::{
    DEFAULT_IPC_PORT, Trial, assert_isolated, engine_env, env_assignment, mono_now,
    trial_ipc_token, valid_unit,
};

const STOP_RPC: Duration = Duration::from_secs(10);
const STOP_AFTER_RPC: Duration = Duration::from_secs(10);
const STOP_TERM: Duration = Duration::from_secs(5);

/// How [`Engine::stop`] finished the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopHow {
    /// The process exited after `StopEngine` answered.
    Rpc,
    /// `StopEngine` did not finish it. SIGTERM did.
    Sigterm,
    /// SIGTERM did not finish it within 5 seconds. SIGKILL did.
    Sigkill,
}

/// Result of [`Engine::stop`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShutdownReport {
    /// `"scope"` when a systemd user scope was used, `"pgroup"` otherwise.
    pub containment: &'static str,
    /// From the stop request until the engine process was gone.
    pub shutdown: Duration,
    /// `StopEngine` returned before the 10 second deadline.
    pub stop_engine_answered: bool,
    pub how: StopHow,
}

/// A running isolated engine.
///
/// Timestamps are `CLOCK_MONOTONIC` nanoseconds ([`crate::mono_now`]).
/// `t_spawn` is taken immediately before the spawn, `t_listen` on the first
/// successful TCP connect to `127.0.0.1:port`, and `t_ready` when
/// `EngineReady` returns.
///
/// Dropping an [`Engine`] without [`Engine::stop`] kills the scope or the
/// process group. It does not call `StopEngine` (Drop cannot wait on RPC).
pub struct Engine {
    /// Engine pid. `exec-counted` is its parent and forwards signals to it.
    pub pid: u32,
    /// systemd unit name without `.scope`, or `None` for the process-group fallback.
    pub scope: Option<String>,
    /// Cgroup directory of [`Self::scope`], when the scope was created.
    pub cgroup: Option<PathBuf>,
    pub t_spawn: u64,
    pub t_listen: u64,
    pub t_ready: u64,
    child: Option<Child>,
    port: u16,
    data: PathBuf,
    reaped: bool,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("pid", &self.pid)
            .field("scope", &self.scope)
            .field("cgroup", &self.cgroup)
            .field("t_spawn", &self.t_spawn)
            .field("t_listen", &self.t_listen)
            .field("t_ready", &self.t_ready)
            .field("port", &self.port)
            .finish()
    }
}

impl Engine {
    /// `"scope"` or `"pgroup"`.
    pub fn containment(&self) -> &'static str {
        if self.scope.is_some() {
            "scope"
        } else {
            "pgroup"
        }
    }

    /// `StopEngine`, then SIGTERM, then SIGKILL. See the module docs.
    pub async fn stop(mut self) -> Result<ShutdownReport> {
        let started = Instant::now();
        let containment = self.containment();
        let answered = request_stop(self.port, &self.data).await;
        let mut how = StopHow::Rpc;
        if answered {
            self.wait_dead(STOP_AFTER_RPC).await;
        }
        if !self.is_dead() {
            how = StopHow::Sigterm;
            self.signal(libc::SIGTERM)?;
            self.wait_dead(STOP_TERM).await;
        }
        if !self.is_dead() {
            how = StopHow::Sigkill;
            self.signal(libc::SIGKILL)?;
            if let (Some(unit), Some(cgroup)) = (self.scope.clone(), self.cgroup.clone()) {
                let _ = write_cgroup_kill(&cgroup, &unit);
            }
            self.wait_dead(Duration::from_secs(2)).await;
        }
        if let Some(unit) = self.scope.clone() {
            systemctl_reset(&unit);
        }
        self.reap().await;
        Ok(ShutdownReport {
            containment,
            shutdown: started.elapsed(),
            stop_engine_answered: answered,
            how,
        })
    }

    fn signal(&self, sig: i32) -> Result<()> {
        if self.pid <= 1 || self.pid == std::process::id() {
            bail!("refusing to signal pid {}", self.pid);
        }
        if crate::detect_real_engine().is_some_and(|engine| engine.pid == self.pid) {
            bail!(
                "refusing to signal the live zeron engine (pid {})",
                self.pid
            );
        }
        if let Some(unit) = &self.scope {
            let cgroup =
                fs::read_to_string(format!("/proc/{}/cgroup", self.pid)).unwrap_or_default();
            if !cgroup.contains(unit.as_str()) {
                bail!("pid {} is not in scope {unit}", self.pid);
            }
            // The scope's own kill reaches grandchildren. The direct signal
            // covers the engine pid when it is still the main process.
            let _ = systemctl_signal(unit, sig);
        } else if let Some(child) = &self.child {
            let ppid = child.id();
            let stat = fs::read_to_string(format!("/proc/{}/stat", self.pid)).unwrap_or_default();
            let Some((parent, pgrp)) = parse_proc_stat(&stat) else {
                bail!("pid {} has no /proc stat; not signaling it", self.pid);
            };
            if parent != ppid && parent != std::process::id() {
                bail!("pid {} is not our child (ppid {parent})", self.pid);
            }
            let mine = unsafe { libc::getpgrp() };
            if pgrp > 1 && pgrp != mine as u32 && pgrp == self.pid {
                let _ = unsafe { libc::kill(-(self.pid as i32), sig) };
            }
        }
        let rc = unsafe { libc::kill(self.pid as i32, sig) };
        if rc == 0 {
            return Ok(());
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        Err(err).with_context(|| format!("signal {sig} to pid {}", self.pid))
    }

    fn is_dead(&mut self) -> bool {
        if let Some(child) = self.child.as_mut()
            && matches!(child.try_wait(), Ok(Some(_)))
        {
            self.reaped = true;
        }
        !pid_alive(self.pid)
    }

    async fn wait_dead(&mut self, budget: Duration) {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            if self.is_dead() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn reap(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if self.reaped {
            return;
        }
        let _ = tokio::task::spawn_blocking(move || {
            // On the process-group path this child is `exec-counted`. It
            // writes `perf.json` after the engine exits. A child that has
            // already exited is collected on the first `try_wait`.
            let deadline = Instant::now() + Duration::from_millis(500);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) if Instant::now() >= deadline => break,
                    Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                    Err(_) => break,
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        })
        .await;
        self.reaped = true;
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if self.reaped && self.child.is_none() {
            return;
        }
        // Same ownership checks as `signal`: never SIGKILL a pid that is not
        // this launch's child or inside this launch's `zb-*` scope. A reused
        // pid (including the live engine) is left alone. `cgroup.kill` is
        // additionally restricted to that scope directory.
        if let (Some(unit), Some(cgroup)) = (&self.scope, &self.cgroup) {
            let _ = write_cgroup_kill(cgroup, unit);
        }
        let _ = self.signal(libc::SIGKILL);
        if let Some(unit) = &self.scope {
            systemctl_reset(unit);
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(300) {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// Prepare `trial`, check the guards, and start `bin`.
///
/// `bin` is the engine executable (`zeron-engine`). The trial's
/// [`Trial::bench_bin`] is the `zeron-bench` that runs `exec-counted`.
pub async fn launch(bin: impl AsRef<Path>, trial: &Trial) -> Result<Engine> {
    let bin = absolute_existing(bin.as_ref())?;
    trial.prepare()?;
    assert_isolated(trial)?;
    // After the guard. The binary is placed inside the trial and the tool
    // names are relative links to it, so a later isolation walk stays inside
    // `ZB_ROOT`. A launch whose `bench_bin` has no `zb-shim` sibling (the
    // scaffold's fake-engine test, before this bin is built) leaves `shims/`
    // empty and the engine resolves commands from `/usr/bin`.
    install_shims(trial)?;
    if trial.port == DEFAULT_IPC_PORT || forbidden_now(trial.port) {
        bail!("refusing to launch on port {}", trial.port);
    }
    // Bind once more immediately before spawn, then release. The engine
    // binds the port itself.
    TcpListener::bind(("127.0.0.1", trial.port))
        .with_context(|| format!("port {} was taken between the guard and spawn", trial.port))?;

    let env = engine_env(trial, &trial.extra);
    let stdout_path = trial.root.join("engine.stdout");
    let stderr_path = trial.root.join("engine.stderr");
    let t_spawn = mono_now();
    let spawned = match spawn_scope(trial, &bin, &env, &stdout_path, &stderr_path) {
        Ok(child) => match wait_for_scope(child, trial, &stderr_path).await {
            Ok(started) => started,
            Err(StartError::Unavailable(reason)) => {
                let _ = fs::write(
                    trial.root.join("scope-fallback.txt"),
                    format!("systemd-run unavailable: {reason}\n"),
                );
                spawn_pgroup(trial, &bin, &env, &stdout_path, &stderr_path)?
            }
            Err(StartError::Failed(err)) => return Err(err),
        },
        Err(err) => {
            let _ = fs::write(
                trial.root.join("scope-fallback.txt"),
                format!("systemd-run spawn failed: {err}\n"),
            );
            spawn_pgroup(trial, &bin, &env, &stdout_path, &stderr_path)?
        }
    };

    let mut spawned = spawned;
    let ready = wait_ready(trial, &mut spawned, &stderr_path).await;
    let (t_listen, t_ready, pid) = match ready {
        Ok(times) => times,
        Err(err) => {
            abandon(&mut spawned);
            return Err(err);
        }
    };
    Ok(Engine {
        pid,
        scope: spawned.scope,
        cgroup: spawned.cgroup,
        t_spawn,
        t_listen,
        t_ready,
        child: Some(spawned.child),
        port: trial.port,
        data: trial.data.clone(),
        reaped: false,
    })
}

/// Tools the bench counts. `PATH` lists [`Trial::shims`] first, so each of
/// these names hits `zb-shim` before `/usr/bin`.
const SHIM_NAMES: &[&str] = &[
    "git", "sh", "bash", "ps", "ss", "lsof", "node", "npm", "npx", "uvx", "cargo", "rustc",
];

/// Install `zb-shim` into the trial and symlink [`SHIM_NAMES`] at it.
///
/// The file inside the trial is a hard link of the built binary when the
/// trial and the target dir share a filesystem, otherwise a copy. The tool
/// names are relative symlinks (`git` → `zb-shim`), so they canonicalize
/// back inside the bench root.
fn install_shims(trial: &Trial) -> Result<()> {
    let Some(source) = shim_source(trial) else {
        return Ok(());
    };
    let dest = trial.shims.join("zb-shim");
    place_shim_binary(&source, &dest)?;
    for name in SHIM_NAMES {
        ensure_rel_symlink(Path::new("zb-shim"), &trial.shims.join(name))?;
    }
    Ok(())
}

fn shim_source(trial: &Trial) -> Option<PathBuf> {
    let candidate = trial.bench_bin.parent()?.join("zb-shim");
    if !candidate.is_file() {
        return None;
    }
    fs::canonicalize(&candidate).ok().or(Some(candidate))
}

fn place_shim_binary(source: &Path, dest: &Path) -> Result<()> {
    if let (Ok(src), Ok(dst)) = (fs::metadata(source), fs::metadata(dest))
        && src.dev() == dst.dev()
        && src.ino() == dst.ino()
    {
        return Ok(());
    }
    if dest.exists() {
        fs::remove_file(dest).with_context(|| format!("replace {}", dest.display()))?;
    }
    if fs::hard_link(source, dest).is_ok() {
        return Ok(());
    }
    fs::copy(source, dest)
        .with_context(|| format!("copy {} to {}", source.display(), dest.display()))?;
    let mode = fs::metadata(source)?.permissions().mode() | 0o111;
    fs::set_permissions(dest, fs::Permissions::from_mode(mode))?;
    Ok(())
}

fn ensure_rel_symlink(rel_target: &Path, link: &Path) -> Result<()> {
    match fs::read_link(link) {
        Ok(existing) if existing == rel_target => return Ok(()),
        Ok(_) => fs::remove_file(link)?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => fs::remove_file(link)?,
    }
    std::os::unix::fs::symlink(rel_target, link)
        .with_context(|| format!("symlink {} -> {}", link.display(), rel_target.display()))?;
    Ok(())
}

/// Host checks for `zeron-bench doctor`: user scope, smaps, perf paranoid,
/// RAPL readability, and the live engine (read-only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoctorReport {
    pub scope_ok: bool,
    pub scope_detail: String,
    pub smaps_ok: bool,
    pub smaps_detail: String,
    pub perf_paranoid: Option<i32>,
    pub perf_detail: String,
    pub rapl_readable: bool,
    pub rapl_detail: String,
    pub real_engine: Option<crate::RealEngine>,
}

impl std::fmt::Display for DoctorReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "scope: {} ({})", mark(self.scope_ok), self.scope_detail)?;
        writeln!(f, "smaps: {} ({})", mark(self.smaps_ok), self.smaps_detail)?;
        let perf = match self.perf_paranoid {
            Some(value) => format!("paranoid={value}"),
            None => "unreadable".into(),
        };
        writeln!(f, "perf: {perf} ({})", self.perf_detail)?;
        writeln!(
            f,
            "rapl: {} ({})",
            if self.rapl_readable {
                "readable"
            } else {
                "unavailable"
            },
            self.rapl_detail
        )?;
        match &self.real_engine {
            Some(engine) => {
                let ports = engine
                    .ports
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(
                    f,
                    "real_engine: pid={} ports={} data={}",
                    engine.pid,
                    if ports.is_empty() { "-".into() } else { ports },
                    engine.data_dir.display()
                )?;
            }
            None => writeln!(f, "real_engine: not detected")?,
        }
        Ok(())
    }
}

pub fn doctor() -> DoctorReport {
    let (scope_ok, scope_detail) = probe_scope();
    let (smaps_ok, smaps_detail) = probe_smaps();
    let (perf_paranoid, perf_detail) = probe_perf();
    let (rapl_readable, rapl_detail) = probe_rapl();
    DoctorReport {
        scope_ok,
        scope_detail,
        smaps_ok,
        smaps_detail,
        perf_paranoid,
        perf_detail,
        rapl_readable,
        rapl_detail,
        real_engine: crate::detect_real_engine(),
    }
}

struct Spawned {
    child: Child,
    scope: Option<String>,
    cgroup: Option<PathBuf>,
}

enum StartError {
    Unavailable(String),
    Failed(anyhow::Error),
}

/// Arguments after `systemd-run`: the scope, `env -i`, and `exec-counted`.
pub(crate) fn scope_argv(trial: &Trial, bin: &Path, env: &[(OsString, OsString)]) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("--user"),
        OsString::from("--scope"),
        OsString::from("--collect"),
        OsString::from("--expand-environment=no"),
        OsString::from(format!("--unit={}", trial.scope_unit)),
        OsString::from("-p"),
        OsString::from(format!("MemoryMax={}", trial.memory_max)),
        OsString::from("-p"),
        OsString::from("TasksMax=4096"),
        OsString::from("--"),
        OsString::from("/usr/bin/env"),
        OsString::from("-i"),
    ];
    for (key, value) in env {
        args.push(env_assignment(key, value));
    }
    args.push(trial.bench_bin.clone().into());
    args.push(OsString::from("exec-counted"));
    args.push(OsString::from("--"));
    args.push(bin.as_os_str().to_os_string());
    args
}

fn spawn_scope(
    trial: &Trial,
    bin: &Path,
    env: &[(OsString, OsString)],
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<Child> {
    if !valid_unit(&trial.scope_unit) {
        bail!("refusing scope unit {}", trial.scope_unit);
    }
    if !Path::new("/usr/bin/systemd-run").is_file() {
        bail!("systemd-run is not installed");
    }
    let stdout = File::create(stdout_path)?;
    let stderr = File::create(stderr_path)?;
    let mut cmd = Command::new("/usr/bin/systemd-run");
    cmd.args(scope_argv(trial, bin, env))
        .current_dir(&trial.home)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    cmd.spawn().context("spawn systemd-run")
}

async fn wait_for_scope(
    mut child: Child,
    trial: &Trial,
    stderr_path: &Path,
) -> Result<Spawned, StartError> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let detail = tail(stderr_path);
                if scope_unavailable(&detail, status) {
                    return Err(StartError::Unavailable(detail));
                }
                return Err(StartError::Failed(anyhow::anyhow!(
                    "systemd-run exited {status}: {detail}"
                )));
            }
            Ok(None) => {}
            Err(err) => return Err(StartError::Failed(err.into())),
        }
        if let Ok(Some(cgroup)) = cgroup_path(&trial.scope_unit) {
            return Ok(Spawned {
                child,
                scope: Some(trial.scope_unit.clone()),
                cgroup: Some(cgroup),
            });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(StartError::Failed(anyhow::anyhow!(
                "scope {} did not appear: {}",
                trial.scope_unit,
                tail(stderr_path)
            )));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn spawn_pgroup(
    trial: &Trial,
    bin: &Path,
    env: &[(OsString, OsString)],
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<Spawned> {
    use std::os::unix::process::CommandExt;
    // Same argv shape as the scope path, without systemd. `exec-counted`
    // stays up to read the counters. Its child, the engine, leads its own
    // process group so a group signal still reaches the tree.
    let stdout = File::create(stdout_path)?;
    let stderr = File::create(stderr_path)?;
    let mut cmd = Command::new(&trial.bench_bin);
    cmd.arg("exec-counted").arg("--").arg(bin);
    cmd.env_clear();
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.current_dir(&trial.home)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .process_group(0);
    let child = cmd.spawn().context("spawn engine in a process group")?;
    Ok(Spawned {
        child,
        scope: None,
        cgroup: None,
    })
}

async fn wait_ready(
    trial: &Trial,
    spawned: &mut Spawned,
    stderr_path: &Path,
) -> Result<(u64, u64, u32)> {
    let deadline = Instant::now() + trial.ready_timeout;
    let addr = format!("127.0.0.1:{}", trial.port);
    let mut t_listen = None;
    let mut pid = None;
    loop {
        if Instant::now() >= deadline {
            bail!(
                "engine was not ready within {}s (port {}): {}",
                trial.ready_timeout.as_secs(),
                trial.port,
                tail(stderr_path)
            );
        }
        if let Some(status) = spawned.child.try_wait()? {
            bail!("engine exited {status} before ready: {}", tail(stderr_path));
        }
        if pid.is_none() {
            pid = engine_pid(spawned, &trial.bench_bin);
        }
        if t_listen.is_none() {
            match TcpStream::connect(&addr).await {
                Ok(_) => t_listen = Some(mono_now()),
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    continue;
                }
            }
        }
        let Some(token) = trial_ipc_token(&trial.data).unwrap_or(None) else {
            tokio::time::sleep(Duration::from_millis(20)).await;
            continue;
        };
        let url = format!("ws://127.0.0.1:{}", trial.port);
        let client = match zeron_rpc::connect_ws(&url, Some(token.as_str())).await {
            Ok(client) => client,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(20)).await;
                continue;
            }
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(
            remaining,
            client.call(zeron_rpc::methods::ENGINE_READY, serde_json::json!({})),
        )
        .await
        {
            Ok(Ok(_)) => {
                let t_listen = t_listen.context("listen time missing")?;
                let pid = pid
                    .or_else(|| engine_pid(spawned, &trial.bench_bin))
                    .or_else(|| {
                        // `/proc/.../children` was unreadable. The wrapper still
                        // forwards signals to the command.
                        spawned.scope.is_none().then(|| spawned.child.id())
                    });
                let Some(pid) = pid else {
                    bail!("engine became ready but its pid was not visible");
                };
                return Ok((t_listen, mono_now(), pid));
            }
            Ok(Err(zeron_rpc::RpcError::Transport(_) | zeron_rpc::RpcError::Closed)) => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok(Err(err)) => bail!("EngineReady: {err}"),
            Err(_) => bail!("EngineReady timed out: {}", tail(stderr_path)),
        }
    }
}

fn engine_pid(spawned: &Spawned, bin_hint: &Path) -> Option<u32> {
    if spawned.scope.is_none() {
        return counted_child(spawned.child.id());
    }
    let cgroup = spawned.cgroup.as_ref()?;
    let text = fs::read_to_string(cgroup.join("cgroup.procs")).ok()?;
    let pids: Vec<u32> = text
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .filter(|pid| *pid > 1)
        .collect();
    let hint = bin_hint.file_name().unwrap_or_default();
    for pid in &pids {
        let cmd = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        if !hint.is_empty() && cmd.windows(hint.len()).any(|w| w == hint.as_bytes()) {
            // This pid is `zeron-bench exec-counted`. The engine is its child.
            return counted_child(*pid);
        }
    }
    None
}

/// Direct child of `exec-counted`. The wrapper forks once, then `execve`s.
fn counted_child(pid: u32) -> Option<u32> {
    let text = fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).ok()?;
    text.split_whitespace().find_map(|field| {
        let child: u32 = field.parse().ok()?;
        (child > 1).then_some(child)
    })
}

async fn request_stop(port: u16, data: &Path) -> bool {
    // Never dial the live engine's port, even if a caller built a bad trial.
    if port == 0 || port == DEFAULT_IPC_PORT || forbidden_now(port) {
        return false;
    }
    let Ok(Some(token)) = trial_ipc_token(data) else {
        return false;
    };
    let url = format!("ws://127.0.0.1:{port}");
    let Ok(client) = zeron_rpc::connect_ws(&url, Some(token.as_str())).await else {
        return false;
    };
    tokio::time::timeout(
        STOP_RPC,
        client.call(zeron_rpc::methods::STOP_ENGINE, serde_json::json!({})),
    )
    .await
    .ok()
    .and_then(|result| result.ok())
    .is_some()
}

fn abandon(spawned: &mut Spawned) {
    if let (Some(unit), Some(cgroup)) = (&spawned.scope, &spawned.cgroup) {
        let _ = write_cgroup_kill(cgroup, unit);
        let _ = systemctl_signal(unit, libc::SIGKILL);
        systemctl_reset(unit);
    }
    let _ = spawned.child.kill();
    let _ = spawned.child.wait();
}

fn forbidden_now(port: u16) -> bool {
    // Re-read the live engine. `assert_isolated` already did; this is the
    // last check on the launch and stop paths.
    if port == DEFAULT_IPC_PORT {
        return true;
    }
    crate::detect_real_engine().is_some_and(|engine| engine.ports.contains(&port))
}

fn absolute_existing(bin: &Path) -> Result<PathBuf> {
    let path = if bin.is_absolute() {
        bin.to_path_buf()
    } else {
        std::env::current_dir()
            .context("current directory")?
            .join(bin)
    };
    if !path.is_file() {
        bail!("engine binary {} is not a file", path.display());
    }
    Ok(path)
}

fn cgroup_path(unit: &str) -> Result<Option<PathBuf>> {
    if !valid_unit(unit) {
        bail!("refusing to query unit {unit}");
    }
    let output = Command::new("systemctl")
        .args([
            "--user",
            "show",
            "-p",
            "ControlGroup",
            "--value",
            &format!("{unit}.scope"),
        ])
        .output()
        .context("systemctl show ControlGroup")?;
    let rel = String::from_utf8_lossy(&output.stdout);
    let rel = rel.trim();
    if rel.is_empty() || !rel.contains(unit) {
        return Ok(None);
    }
    let path = Path::new("/sys/fs/cgroup").join(rel.trim_start_matches('/'));
    if cgroup_kill_path_ok(&path, unit) {
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

fn systemctl_signal(unit: &str, sig: i32) -> Result<()> {
    if !valid_unit(unit) {
        bail!("refusing to systemctl kill {unit}");
    }
    let name = match sig {
        libc::SIGTERM => "SIGTERM",
        libc::SIGKILL => "SIGKILL",
        other => bail!("refusing to send signal {other} through systemctl"),
    };
    let status = Command::new("systemctl")
        .args([
            "--user",
            "kill",
            &format!("{unit}.scope"),
            &format!("--signal={name}"),
        ])
        .status()
        .context("systemctl kill")?;
    if status.success() {
        Ok(())
    } else {
        bail!("systemctl kill {unit} exited {status}");
    }
}

fn systemctl_reset(unit: &str) {
    if !valid_unit(unit) {
        return;
    }
    let unit = format!("{unit}.scope");
    let _ = Command::new("systemctl")
        .args(["--user", "stop", &unit])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = Command::new("systemctl")
        .args(["--user", "reset-failed", &unit])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub(crate) fn write_cgroup_kill(path: &Path, unit: &str) -> Result<()> {
    if !cgroup_kill_path_ok(path, unit) {
        bail!("refusing to write cgroup.kill at {}", path.display());
    }
    let meta = fs::symlink_metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        bail!("{} is not a cgroup directory", path.display());
    }
    let canon = fs::canonicalize(path)?;
    if !cgroup_kill_path_ok(&canon, unit) {
        bail!("refusing to write cgroup.kill at {}", canon.display());
    }
    let kill = canon.join("cgroup.kill");
    let mut file = File::options().write(true).open(&kill)?;
    file.write_all(b"1\n")?;
    Ok(())
}

pub(crate) fn cgroup_kill_path_ok(path: &Path, unit: &str) -> bool {
    if !valid_unit(unit) {
        return false;
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return false;
    }
    let root = Path::new("/sys/fs/cgroup");
    if path == root || !path.starts_with(root) {
        return false;
    }
    let scoped = OsString::from(format!("{unit}.scope"));
    path.components().any(|c| c.as_os_str() == scoped)
}

pub(crate) fn parse_proc_stat(text: &str) -> Option<(u32, u32)> {
    let rest = text.rsplit_once(')')?.1.trim_start();
    let mut fields = rest.split_whitespace();
    let _state = fields.next()?;
    let ppid = fields.next()?.parse().ok()?;
    let pgrp = fields.next()?.parse().ok()?;
    Some((ppid, pgrp))
}

fn pid_alive(pid: u32) -> bool {
    if pid <= 1 {
        return false;
    }
    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

fn scope_unavailable(stderr: &str, status: ExitStatus) -> bool {
    let text = stderr.to_ascii_lowercase();
    text.contains("failed to connect to bus")
        || text.contains("failed to create bus connection")
        || text.contains("no medium found")
        || status.code() == Some(127)
}

fn tail(path: &Path) -> String {
    let Ok(bytes) = fs::read(path) else {
        return String::new();
    };
    let start = bytes.len().saturating_sub(1500);
    String::from_utf8_lossy(&bytes[start..]).trim().to_string()
}

fn probe_scope() -> (bool, String) {
    let unit = format!(
        "zb-doctor-{}-{}",
        std::process::id(),
        mono_now() % 1_000_000_000
    );
    if !valid_unit(&unit) {
        return (false, "internal unit name was rejected".into());
    }
    let output = Command::new("/usr/bin/systemd-run")
        .args([
            "--user",
            "--scope",
            "--collect",
            "--quiet",
            "--expand-environment=no",
            &format!("--unit={unit}"),
            "--",
            "/usr/bin/true",
        ])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            systemctl_reset(&unit);
            (true, "transient user scope".into())
        }
        Ok(output) => (
            false,
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ),
        Err(err) => (false, err.to_string()),
    }
}

fn probe_smaps() -> (bool, String) {
    let path = "/proc/self/smaps_rollup";
    match fs::read_to_string(path) {
        Ok(text) if text.contains("Pss:") => (true, path.into()),
        Ok(_) => (false, format!("{path} has no Pss field")),
        Err(err) => (false, format!("{path}: {err}")),
    }
}

fn probe_perf() -> (Option<i32>, String) {
    let path = "/proc/sys/kernel/perf_event_paranoid";
    match fs::read_to_string(path) {
        Ok(text) => match text.trim().parse::<i32>() {
            Ok(value) if value <= 2 => (
                Some(value),
                "user-space instructions and cycles are allowed for this user".into(),
            ),
            Ok(value) => (
                Some(value),
                "user-space instruction counts need paranoid <= 2".into(),
            ),
            Err(_) => (None, format!("could not parse {path}")),
        },
        Err(err) => (None, format!("{path}: {err}")),
    }
}

fn probe_rapl() -> (bool, String) {
    let path = "/sys/class/powercap/intel-rapl:0/energy_uj";
    // Open and close. Do not read the counter: the value is a side channel
    // and doctor only needs to know whether a later sample could.
    match File::open(path) {
        Ok(_) => (true, path.into()),
        Err(err) => (false, format!("{path}: {err}")),
    }
}

fn mark(ok: bool) -> &'static str {
    if ok { "ok" } else { "unavailable" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::test_trial;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn scope_argv_is_env_dash_i_and_carries_no_parent_secrets() {
        let trial = test_trial("argv");
        let env = engine_env(
            &trial,
            &[
                ("ZB_REPLAY_MODE", "mitm"),
                ("ZERON_SESSION_IDLE_MS", "20000"),
            ],
        );
        let bin = Path::new("/usr/bin/true");
        let args = scope_argv(&trial, bin, &env);
        let text = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("--scope"), "{text}");
        assert!(
            text.contains(&format!("--unit={}", trial.scope_unit)),
            "{text}"
        );
        assert!(text.contains(&format!("MemoryMax={}", trial.memory_max)));
        assert!(text.contains("TasksMax=4096"));
        assert!(text.contains("/usr/bin/env"));
        assert!(args.iter().any(|arg| arg == "-i"));
        assert!(text.contains("exec-counted"));
        assert!(text.contains("/usr/bin/true"));
        assert!(!text.contains("ZERON_CHAT_ID"));
        assert!(!text.contains("ZERON_IPC_TOKEN"));
        assert!(!text.contains("CLAUDECODE"));
        assert!(!text.contains("CLAUDE_CODE_ENTRYPOINT"));
        assert!(!text.contains("RUST_LOG"));
        assert!(text.contains(&format!("ZERON_DATA_DIR={}", trial.data.display())));
        assert!(text.contains("ZERON_SESSION_IDLE_MS=20000"));
        assert!(text.contains("ZB_REPLAY_MODE=mitm"));
    }

    #[test]
    fn env_i_round_trip_drops_parent_variables() {
        let _guard = crate::env::lock_process_env();
        let keys = ["ZERON_IPC_TOKEN", "ZERON_CHAT_ID"];
        let saved: Vec<_> = keys
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        struct Restore(Vec<(&'static str, Option<OsString>)>);
        impl Drop for Restore {
            fn drop(&mut self) {
                for (key, value) in self.0.drain(..) {
                    match value {
                        Some(value) => crate::env::set_process_var(key, value),
                        None => crate::env::unset_process_var(key),
                    }
                }
            }
        }
        let _restore = Restore(saved);
        crate::env::set_process_var("ZERON_IPC_TOKEN", "parent-secret");
        crate::env::set_process_var("ZERON_CHAT_ID", "parent-chat");
        let trial = test_trial("env-i");
        let env = engine_env(&trial, &[] as &[(&str, &str)]);
        let mut cmd = Command::new("/usr/bin/env");
        cmd.arg("-i");
        for (key, value) in &env {
            cmd.arg(env_assignment(key, value));
        }
        cmd.arg("/usr/bin/env");
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(!text.contains("ZERON_IPC_TOKEN"), "leak: {text}");
        assert!(!text.contains("ZERON_CHAT_ID"), "leak: {text}");
        assert!(!text.contains("parent-secret"));
        assert!(text.contains(&format!("ZERON_IPC_PORT={}\n", trial.port)));
        assert!(text.contains(&format!("HOME={}\n", trial.home.display())));
        assert!(!text.lines().any(|line| line.starts_with("RUST_LOG=")));
    }

    #[test]
    fn cgroup_kill_rejects_paths_that_are_not_our_scope() {
        assert!(!cgroup_kill_path_ok(
            Path::new("/tmp/zb-foo-0.scope"),
            "zb-foo-0"
        ));
        assert!(!cgroup_kill_path_ok(
            Path::new("/sys/fs/cgroup"),
            "zb-foo-0"
        ));
        assert!(!cgroup_kill_path_ok(
            Path::new("/sys/fs/cgroup/user.slice"),
            "zb-foo-0"
        ));
        assert!(!cgroup_kill_path_ok(
            Path::new("/sys/fs/cgroup/../../tmp/zb-foo-0.scope"),
            "zb-foo-0"
        ));
        assert!(!cgroup_kill_path_ok(
            Path::new("/sys/fs/cgroup/user.slice/user-1.slice/app.slice/notzb.scope"),
            "user"
        ));
        assert!(cgroup_kill_path_ok(
            Path::new(
                "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/zb-run-0.scope"
            ),
            "zb-run-0"
        ));
    }

    #[test]
    fn parse_proc_stat_reads_ppid_and_pgrp_past_the_comm_field() {
        assert_eq!(parse_proc_stat("12 (my proc) S 1 12 12 0"), Some((1, 12)));
        assert_eq!(parse_proc_stat("1 (systemd) S 0 1"), Some((0, 1)));
        assert_eq!(parse_proc_stat("nope"), None);
    }

    #[test]
    fn install_shims_links_every_counted_command_inside_the_trial() {
        let mut trial = test_trial("shims");
        let bin_dir = trial.root.join("bins");
        fs::create_dir_all(&bin_dir).unwrap();
        let source = bin_dir.join("zb-shim");
        fs::write(&source, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
        trial.bench_bin = bin_dir.join("zeron-bench");
        install_shims(&trial).unwrap();
        install_shims(&trial).unwrap();
        let root = fs::canonicalize(&trial.zb_root).unwrap();
        for name in SHIM_NAMES {
            let link = trial.shims.join(name);
            assert_eq!(
                fs::read_link(&link).unwrap(),
                Path::new("zb-shim"),
                "{name}"
            );
            let canon = fs::canonicalize(&link).unwrap();
            assert!(canon.starts_with(&root), "{}", canon.display());
        }
        assert_isolated(&trial).unwrap();
    }

    #[test]
    fn install_shims_leaves_the_directory_empty_without_a_binary() {
        let mut trial = test_trial("no-shim");
        trial.bench_bin = trial.root.join("absent").join("zeron-bench");
        install_shims(&trial).unwrap();
        let entries: Vec<_> = fs::read_dir(&trial.shims).unwrap().collect();
        assert!(entries.is_empty());
    }
}
