//! The desktop app's background engine: started by the app when nothing
//! serves its port, it outlives the app's window — close or quit the app and
//! sessions keep running — and shows up as its own process ("Zeron Engine"
//! in Activity Monitor, `zeron-engine` elsewhere).
//!
//! It is started as a detached child of the app rather than through a
//! service manager. On macOS that keeps privacy permissions (Files &
//! Folders, Screen Recording, Microphone) attributed to the app, as they
//! were with the in-process engine, and needs no background-item approval;
//! everywhere it works the same way. Nothing restarts it behind the app's
//! back: the app starts it again the next time it opens (or when its
//! connection drops). After an app update it restarts into the new version
//! once nothing is running — on macOS only while the app runs, by stopping
//! so the app starts the new one as its child; elsewhere by relaunching
//! itself. An update asked of it (`ApplyUpdate`, from any device) it carries
//! out itself, app or no app: on macOS it stops, then becomes the new
//! version in the same process. It only stops when it will, and if the new
//! version still doesn't start it becomes the installed one again.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use zeron_rpc::{RpcError, RpcReply, RpcService, methods};

use crate::{Engine, EngineConfig, InstanceLock, WorkspaceScope, deferred, desktop_notify};

// Starting the engine helper is shared with the command line and the zgx
// app, which don't link the engine (crates/launch).
pub use zeron_launch::{
    APP_ENV, ENGINE_EXECUTABLES, ENGINE_SUBCOMMAND, MANAGED_ENV, app_path, engine_command,
    reap, spawn_detached,
};

/// How long a restarting engine waits for its predecessor to let go of the
/// data directory.
const HANDOFF: Duration = Duration::from_secs(60);

/// Start the background engine for `config`, detached from the app
/// ([`zeron_launch::launch`]).
pub fn launch(config: &EngineConfig, app_exe: &Path) -> std::io::Result<Child> {
    zeron_launch::launch(
        &zeron_launch::LaunchConfig {
            data_dir: config.data_dir.clone(),
            ipc_port: config.ipc_port,
            edge_url: config.edge_url.clone(),
            workos_client_id: config.workos_client_id.clone(),
            edge_token: config.edge_token.clone(),
            org_id: config.org_id.clone(),
        },
        app_exe,
    )
}

/// IPC-only lifecycle control: `StopEngine` ends the background engine (the
/// app's sign-out and workspace switches restart it on the new profile).
struct ManagedRpc {
    inner: Arc<dyn RpcService>,
    stop_tx: tokio::sync::mpsc::UnboundedSender<()>,
}

#[async_trait]
impl RpcService for ManagedRpc {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        if method != methods::STOP_ENGINE {
            return self.inner.handle(method, params).await;
        }
        let stop_tx = self.stop_tx.clone();
        // Let the reply reach the client before the server is aborted.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let _ = stop_tx.send(());
        });
        RpcReply::value(&serde_json::json!({ "ok": true }))
    }
}

/// Take the data directory, waiting out a predecessor that is still
/// draining (a restart into a new version starts the replacement first).
async fn acquire_lock(data_dir: &Path) -> anyhow::Result<InstanceLock> {
    let deadline = std::time::Instant::now() + HANDOFF;
    loop {
        match InstanceLock::acquire(data_dir) {
            Ok(lock) => return Ok(lock),
            Err(err) if std::time::Instant::now() >= deadline => return Err(err.into()),
            Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }
}

/// The executable a relaunch runs: this one's name where the installation
/// now keeps it — not this process's own image, which an update leaves
/// behind (a Linux install's previous version directory; Windows renames the
/// running image aside).
fn installed_executable(exe: &Path, install: &zeron_update::InstallKind) -> PathBuf {
    let name = exe.file_name().unwrap_or_default();
    match install {
        zeron_update::InstallKind::Managed { app_root } => app_root.join("current").join(name),
        zeron_update::InstallKind::MacApp { bundle } => {
            bundle.join("Contents").join("MacOS").join(name)
        }
        #[cfg(windows)]
        zeron_update::InstallKind::WindowsPortable { directory } => directory.join("zeron.exe"),
        _ => exe.to_path_buf(),
    }
}

/// The binary agents launch as their Zeron MCP server: this process's own
/// image while it is still there, else the installation's current copy. An
/// image an update replaced, or a sandbox moved aside, names a path the
/// agent's next spawn of the server can't run (Linux even reports a deleted
/// one as `"<path> (deleted)"`), and an agent whose server won't start has
/// no Zeron tools for the rest of its session.
pub(crate) fn mcp_executable() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let raw = exe.to_string_lossy();
    let deleted = raw.ends_with(" (deleted)");
    let exe = match raw.strip_suffix(" (deleted)") {
        Some(path) => PathBuf::from(path),
        None => exe.clone(),
    };
    if !deleted && exe.exists() {
        return Some(exe);
    }
    let installed = installed_executable(&exe, &zeron_update::detect_install());
    tracing::info!(stale = %exe.display(), installed = %installed.display(), "this engine's image moved; agents launch the installed Zeron MCP");
    installed.exists().then_some(installed)
}

/// The app was moved to the Trash or uninstalled: once nothing is running,
/// stop — an engine left running from a deleted app would keep the Trash
/// from emptying ("in use") with no app left to quit it. Updates keep the
/// installed path, so they never trigger this.
fn stop_when_uninstalled(
    runtime: &crate::EngineRuntime,
    stop: tokio::sync::mpsc::UnboundedSender<()>,
) {
    const CHECK: Duration = Duration::from_secs(30);
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let installed = installed_executable(&exe, &zeron_update::detect_install());
    let sessions = runtime.core().sessions.clone();
    let terminals = runtime.core().terminals.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(CHECK).await;
            if installed.exists() || sessions.any_active() || terminals.any_open() {
                continue;
            }
            tracing::info!(path = %installed.display(), "the app is gone; stopping the background engine");
            let _ = stop.send(());
            return;
        }
    });
}

/// Whether the app that starts this engine runs now. On macOS an engine
/// the app starts is its child and attributed to it — Keychain and privacy
/// permissions included. One this engine relaunched itself (orphaned when
/// this one exits) was found outside the user's Keychain after an update:
/// Claude Code reported "Not logged in" and sign-ins failed until the app
/// started the engine again. So on macOS a restart into a new version only
/// happens while the app runs, and leaves the start to it (its reconnect
/// starts the engine when the port goes quiet); until then this engine keeps
/// serving — the running image outlives the update on disk.
#[cfg(target_os = "macos")]
fn app_starts_engine() -> bool {
    let Some(bundle) = std::env::var_os(APP_ENV).map(PathBuf::from) else {
        return false;
    };
    app_running(&bundle.join("Contents").join("MacOS"))
}

/// A process runs from `dir` (the bundle's `Contents/MacOS`) that isn't one
/// of the engine's own executables: the app.
#[cfg(target_os = "macos")]
fn app_running(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt as _;
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let mut pids = vec![0 as libc::pid_t; 8192];
    let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
    // SAFETY: the buffer is `bytes` long; the call writes at most that.
    let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    for &pid in pids.iter().take(count.max(0) as usize) {
        if pid <= 0 {
            continue;
        }
        // SAFETY: `path` is PROC_PIDPATHINFO_MAXSIZE bytes, as the call needs.
        let len = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
        if len <= 0 {
            continue;
        }
        let exe = Path::new(std::ffi::OsStr::from_bytes(&path[..len as usize]));
        let is_engine = exe
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| ENGINE_EXECUTABLES.contains(&name));
        if exe.parent() == Some(dir.as_path()) && !is_engine {
            return true;
        }
    }
    false
}

/// Relaunch this engine with its own arguments and environment; the new one
/// waits for this one to release the data directory.
fn relaunch_self() -> anyhow::Result<()> {
    let exe = installed_executable(&std::env::current_exe()?, &zeron_update::detect_install());
    let mut command = Command::new(exe);
    command.args(std::env::args_os().skip(1));
    let child = spawn_detached(&mut command)?;
    reap(child);
    Ok(())
}

impl Engine {
    /// Run as the desktop app's background engine until stopped: serves the
    /// app over the IPC port at once (identity and sign-in before the profile
    /// is open, so a session still picking an organization waits for the
    /// app's onboarding instead of a terminal prompt), posts desktop banners,
    /// and restarts into a newer installed version once nothing is running.
    /// Ends on `StopEngine`, SIGTERM/ctrl-c, or — for a synced workspace —
    /// sign-out, after draining.
    pub async fn run_managed(self) -> anyhow::Result<()> {
        let config = self.config;
        tracing::info!(data_dir = %config.data_dir.display(), port = config.ipc_port, "background engine starting");
        std::fs::create_dir_all(&config.data_dir)?;
        crate::update_trial::boot(&config.data_dir)?;
        let lock = acquire_lock(&config.data_dir).await?;
        desktop_notify::enable();

        let mut engine = deferred::DeferredEngine::prepare(&config).await?;
        let (stop_tx, mut stop_rx) = tokio::sync::mpsc::unbounded_channel();
        let service: Arc<dyn RpcService> = Arc::new(ManagedRpc {
            inner: engine.service.clone(),
            stop_tx: stop_tx.clone(),
        });
        // The app is waiting on this port; bind as soon as the data dir is
        // ours, retrying while a predecessor's listener closes.
        let server = {
            let deadline = std::time::Instant::now() + HANDOFF;
            loop {
                match crate::serve_ipc(config.ipc_port, &config.data_dir, service.clone()).await {
                    Ok(server) => break server,
                    Err(err) if std::time::Instant::now() >= deadline => return Err(err.into()),
                    Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
                }
            }
        };

        let restart_stop = stop_tx.clone();
        let stopped_for_update = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // macOS: the new version is only ever started by the app (see
        // `app_starts_engine`), so the restart waits for the app to run and
        // this engine keeps serving the old version until then.
        #[cfg(target_os = "macos")]
        zeron_update::set_managed_restart_ready(app_starts_engine);
        zeron_update::set_managed_restart(managed_restart(
            config.data_dir.clone(),
            || std::env::current_exe().ok(),
            restart_stop,
            stopped_for_update.clone(),
        ));
        let uninstalled_stop = stop_tx.clone();
        // Signed in with no organization chosen, there's no profile to open
        // (maybe for a long while), and so nothing a trial could migrate:
        // answering on the port is all it can prove.
        if !engine.profile_is_resolved {
            crate::update_trial::commit_without_a_profile(&config);
        }
        let trial_config = config.clone();
        let boot = engine.spawn_boot(
            config.clone(),
            lock,
            Some(config.ipc_port),
            move |runtime| {
                if let Some(updater) = runtime.core().updater() {
                    updater.restart_when_superseded();
                }
                stop_when_uninstalled(runtime, uninstalled_stop);
                crate::update_trial::prove_and_commit(runtime.core(), &trial_config);
            },
        );

        let mut auth_state = engine.auth.watch_state();
        let mut assembly = engine.state();
        let synced = engine.workspace_scope == WorkspaceScope::Synced;
        // The stop came through `stop_rx` (a restart, `StopEngine`), not a
        // signal or a sign-out: only then may an update bring it back.
        let mut stop_requested = false;
        let outcome = tokio::select! {
            result = crate::shutdown_signal() => result.map_err(anyhow::Error::from),
            _ = stop_rx.recv() => {
                tracing::info!("background engine stop requested");
                stop_requested = true;
                Ok(())
            }
            _ = crate::wait_for_signed_out(&mut auth_state), if synced => {
                tracing::info!("signed out; stopping the synced background engine");
                // A brief reply window for the SignOut RPC itself.
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok(())
            }
            failure = async {
                match deferred::wait(&mut assembly).await {
                    Ok(()) => std::future::pending().await,
                    Err(message) => message,
                }
            } => Err(anyhow::anyhow!("engine assembly failed: {failure}")),
        };
        tracing::info!("background engine shutting down");
        server.abort();
        boot.abort();
        // Before the data dir is released (with the runtime): no refresh may
        // be mid-exchange when the next engine takes over the session file.
        engine.refresh_task.abort();
        engine.auth.quiesce_refresh(Duration::from_secs(10)).await;
        let runtime = engine.runtime.lock().await.take();
        // The app signs out, then stops the engine: the StopEngine may land
        // before the auth watcher does, so the runtime's shutdown decides
        // revival from the auth state, not from the branch that fired.
        if let Some(runtime) = runtime {
            runtime.shutdown().await;
        }
        // macOS: the engine never starts its successor as a new process, so an
        // update asked of it happens here, in this one, with the data directory
        // let go. Without an update this does nothing. (Elsewhere a relaunch has
        // already started the process that does this on its way up.)
        // Only a stop through `stop_rx` carries an update out: one by signal or
        // sign-out stays a stop, and the next start settles any record.
        #[cfg(target_os = "macos")]
        {
            let booted = if stop_requested {
                crate::update_trial::boot(&config.data_dir)
            } else {
                Ok(())
            };
            // Still here after stopping for an update: nothing was handed over
            // and, the app closed, nothing would start an engine again. Become
            // the installed engine instead — the same process, so macOS still
            // attributes it to the app.
            let for_update = stopped_for_update.load(std::sync::atomic::Ordering::SeqCst);
            if comes_back(for_update, stop_requested) {
                if let Err(err) = &booted {
                    tracing::error!(error = %format!("{err:#}"), "the update couldn't start");
                }
                match after_a_failed_update(&config.data_dir) {
                    AfterFailedUpdate::Exec => return Err(exec_installed().into()),
                    AfterFailedUpdate::LeaveIt => {
                        tracing::info!("another engine has the update in hand; leaving it");
                        return outcome;
                    }
                }
            }
            booted?;
        }
        #[cfg(not(target_os = "macos"))]
        let _ = stop_requested;
        outcome
    }
}

/// The restart the updater asks of the background engine, both for an
/// `ApplyUpdate` (which records the update) and to follow a newer app the app
/// installed itself (`SupersededRestart`; no record).
///
/// macOS, with an update in progress: once stopped this process becomes the
/// new version itself (the `boot` after shutdown), app or no app. It stops
/// only if it will; otherwise it refuses, and the update is reported failed
/// with this engine still serving. With no update in progress: stop, and the
/// app starts the newer engine as its child, as always. Elsewhere: relaunch.
fn managed_restart(
    data_dir: PathBuf,
    exe: impl Fn() -> Option<PathBuf> + Send + Sync + 'static,
    stop: tokio::sync::mpsc::UnboundedSender<()>,
    stopped_for_update: Arc<std::sync::atomic::AtomicBool>,
) -> impl Fn() -> anyhow::Result<()> + Send + Sync + 'static {
    use zeron_update::transaction;
    move || {
        if cfg!(target_os = "macos") {
            if transaction::read(&data_dir).is_some_and(|update| update.in_progress()) {
                let exe = exe();
                let here = |step: fn(&Path, &Path) -> bool| {
                    exe.as_deref().is_some_and(|exe| step(&data_dir, exe))
                };
                if here(transaction::superseded_from) {
                    // The app installed a newer version since the update was
                    // asked for: follow it as below; the next start's boot
                    // settles the record.
                    tracing::info!("restarting into a newer app: the app starts the new engine");
                } else {
                    stop_for_update(here(transaction::starts_from))?;
                    stopped_for_update.store(true, std::sync::atomic::Ordering::SeqCst);
                    tracing::info!("restarting into the update: this engine becomes the new one");
                }
            } else {
                // Not flagged: the app starts the new engine, and a backstop
                // here would race it for the data directory and the port.
                tracing::info!("restarting into the update: the app starts the new engine");
            }
        } else {
            relaunch_self()?;
        }
        let _ = stop.send(());
        Ok(())
    }
}

/// After stopping for an update, whether this engine brings itself back
/// rather than exit: only when it was that stop, not a signal or a sign-out
/// that won the race.
#[cfg(target_os = "macos")]
fn comes_back(stopped_for_update: bool, stop_requested: bool) -> bool {
    stopped_for_update && stop_requested
}

/// Stop for an update only when this engine carries it out once stopped.
fn stop_for_update(starts_here: bool) -> anyhow::Result<()> {
    anyhow::ensure!(
        starts_here,
        "this engine can't start the new version itself, so it kept running the current one"
    );
    Ok(())
}

/// What an engine does when an update it stopped for didn't start.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterFailedUpdate {
    /// Become the installed engine.
    Exec,
    /// Exit: another engine has the data directory or the update in hand.
    LeaveIt,
}

/// After an update this engine stopped for didn't start.
///
/// - The data directory is free: a request still waiting is failed (and
///   checked to be) or the record is already finished, and this engine comes
///   back. A trial or restore in progress belongs to another process: leave it.
/// - It's held, stamped with this process's pid: this engine's own lock,
///   kept by something of its core still alive after shutdown (flock is per
///   open file, so it blocks its owner too). Come back without touching the
///   record: the exec closes that close-on-exec file, freeing the lock, and
///   the fresh start's boot carries the update out.
/// - Held by another engine (the app started one, maybe mid-backup for this
///   very update): that engine is up and the record is its business.
#[cfg(target_os = "macos")]
fn after_a_failed_update(data_dir: &Path) -> AfterFailedUpdate {
    use zeron_update::transaction::{self, Phase};
    let lock = match InstanceLock::acquire(data_dir) {
        Ok(lock) => lock,
        Err(_) if stamped_by_this_process(data_dir) => {
            tracing::warn!("this engine still holds its own data-directory lock; coming back");
            return AfterFailedUpdate::Exec;
        }
        Err(_) => return AfterFailedUpdate::LeaveIt,
    };
    let after = match transaction::read(data_dir) {
        None => AfterFailedUpdate::Exec,
        Some(update) if !update.in_progress() => AfterFailedUpdate::Exec,
        Some(update) if update.phase == Phase::Requested => {
            transaction::fail_request(
                data_dir,
                &update.id,
                "Zeron couldn't start the new version, so it stayed on this one.".into(),
            );
            let failed = transaction::read(data_dir)
                .is_some_and(|now| now.id == update.id && now.phase == Phase::Failed);
            if failed { AfterFailedUpdate::Exec } else { AfterFailedUpdate::LeaveIt }
        }
        Some(_) => AfterFailedUpdate::LeaveIt,
    };
    if after == AfterFailedUpdate::Exec {
        tracing::error!("the update didn't start; coming back on the installed version");
    }
    drop(lock);
    after
}

/// The data directory's lock file names this process as its holder (every
/// engine stamps its pid on taking the lock).
#[cfg(target_os = "macos")]
fn stamped_by_this_process(data_dir: &Path) -> bool {
    std::fs::read_to_string(data_dir.join("engine.lock"))
        .is_ok_and(|holder| holder.trim() == std::process::id().to_string())
}

/// Become the installed engine with the same arguments: the same process, so
/// macOS still attributes it to the app. Returns only if that couldn't start.
#[cfg(target_os = "macos")]
fn exec_installed() -> std::io::Error {
    use std::os::unix::process::CommandExt as _;
    let exe = match std::env::current_exe() {
        Ok(exe) => installed_executable(&exe, &zeron_update::detect_install()),
        Err(err) => return err,
    };
    Command::new(exe).args(std::env::args_os().skip(1)).exec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relaunches_run_the_installed_version() {
        let exe = Path::new("/home/me/.zeron/app/0.2.114/zeron-engine");
        assert_eq!(
            installed_executable(
                exe,
                &zeron_update::InstallKind::Managed {
                    app_root: PathBuf::from("/home/me/.zeron/app"),
                }
            ),
            PathBuf::from("/home/me/.zeron/app/current/zeron-engine")
        );
        assert_eq!(
            installed_executable(
                Path::new("/Applications/Zeron.app/Contents/MacOS/Zeron Engine"),
                &zeron_update::InstallKind::MacApp {
                    bundle: PathBuf::from("/Applications/Zeron.app"),
                }
            ),
            PathBuf::from("/Applications/Zeron.app/Contents/MacOS/Zeron Engine")
        );
        assert_eq!(
            installed_executable(exe, &zeron_update::InstallKind::Unmanaged),
            exe.to_path_buf()
        );
    }

    /// `/Applications`-like folder with the installed `Zeron.app` at 1.0.0
    /// (its engine executable included) and a data directory.
    #[cfg(target_os = "macos")]
    struct MacInstall {
        _root: tempfile::TempDir,
        data: PathBuf,
        bundle: PathBuf,
        engine: PathBuf,
    }

    #[cfg(target_os = "macos")]
    impl MacInstall {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let base = std::fs::canonicalize(root.path()).unwrap();
            let bundle = base.join("Applications/Zeron.app");
            let engine = bundle.join("Contents/MacOS/Zeron Engine");
            std::fs::create_dir_all(engine.parent().unwrap()).unwrap();
            std::fs::write(&engine, "").unwrap();
            std::fs::write(
                bundle.join("Contents/Info.plist"),
                "<plist><key>ZeronVersion</key><string>1.0.0</string></plist>",
            )
            .unwrap();
            let data = base.join(".zeron");
            std::fs::create_dir_all(&data).unwrap();
            Self { _root: root, data, bundle, engine }
        }

        /// What `ApplyUpdate` records: 1.0.0 → 2.0.0, into a bundle beside.
        fn request_update(&self) {
            let incoming = self.bundle.with_file_name(".Zeron.app.new-next");
            std::fs::create_dir_all(incoming.join("Contents/MacOS")).unwrap();
            zeron_update::transaction::request(
                &self.data,
                zeron_update::transaction::Layout::MacBundle {
                    bundle: self.bundle.clone(),
                    incoming,
                },
                "1.0.0",
                "2.0.0",
                zeron_update::transaction::RestartMode::Relaunch,
            )
            .unwrap();
        }

        /// The background engine's restart, as the updater calls it, run from
        /// `exe`: whether it went ahead, whether it stopped the engine, and
        /// whether it flagged the stop as one for an update.
        fn restart_from(&self, exe: &Path) -> (bool, bool, bool) {
            let (stop, mut stopped) = tokio::sync::mpsc::unbounded_channel();
            let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let exe = exe.to_path_buf();
            let restart =
                managed_restart(self.data.clone(), move || Some(exe.clone()), stop, flag.clone());
            let went_ahead = restart().is_ok();
            (
                went_ahead,
                stopped.try_recv().is_ok(),
                flag.load(std::sync::atomic::Ordering::SeqCst),
            )
        }
    }

    /// The app installed a newer bundle itself ("Restart to update",
    /// install-on-quit): no update record. The engine still stops for
    /// `SupersededRestart`, unflagged, so the app starts the new engine — the
    /// review of #1345 caught the gate refusing this, which left the new app
    /// on the old engine after every local update.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_newer_app_installed_by_the_app_still_stops_the_engine() {
        let install = MacInstall::new();
        let elsewhere = install.data.join("not-the-installation/Zeron Engine");
        for exe in [&install.engine, &elsewhere] {
            assert_eq!(install.restart_from(exe), (true, true, false), "{}", exe.display());
        }
    }

    /// `ApplyUpdate`, with this engine the installed one (from its bundle, or
    /// from an earlier update's renamed incoming one): it stops, flagged, and
    /// becomes the new version after shutdown.
    #[cfg(target_os = "macos")]
    #[test]
    fn an_update_this_engine_carries_out_stops_it_flagged() {
        let install = MacInstall::new();
        install.request_update();
        let earlier = install.bundle.with_file_name(".Zeron.app.new-earlier");
        for exe in [install.engine.clone(), earlier.join("Contents/MacOS/Zeron Engine")] {
            assert_eq!(install.restart_from(&exe), (true, true, true), "{}", exe.display());
        }
    }

    /// `ApplyUpdate` this engine wouldn't carry out once stopped (not the
    /// installation's): it refuses and keeps serving, and the update fails.
    #[cfg(target_os = "macos")]
    #[test]
    fn an_update_this_engine_cant_start_never_stops_it() {
        let install = MacInstall::new();
        install.request_update();
        let beside = install.data.join("build/Zeron Engine");
        assert_eq!(install.restart_from(&beside), (false, false, false));
        let refused = stop_for_update(false).unwrap_err().to_string();
        assert!(refused.contains("kept running"), "{refused}");
    }

    /// The backstop only touches the record with the data directory held:
    /// another engine holding it (the app's, maybe backing up for this very
    /// update) keeps the record, and this one doesn't come back over it.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_backstop_leaves_another_engines_update_alone() {
        use zeron_update::transaction::{self, Phase};
        let install = MacInstall::new();
        install.request_update();
        let other = InstanceLock::acquire(&install.data).unwrap();
        // Another engine's stamp (a lock held here stands in for its flock).
        std::fs::write(install.data.join("engine.lock"), "999999").unwrap();
        assert_eq!(after_a_failed_update(&install.data), AfterFailedUpdate::LeaveIt);
        assert_eq!(transaction::read(&install.data).unwrap().phase, Phase::Requested);
        drop(other);
        assert_eq!(after_a_failed_update(&install.data), AfterFailedUpdate::Exec);
        assert_eq!(transaction::read(&install.data).unwrap().phase, Phase::Failed);
    }

    /// Second review of #1345: the engine's own lock, still held after
    /// shutdown by something of its core, blocks it like another engine's
    /// (flock is per open file). It still comes back — the exec frees that
    /// lock — and leaves the record for the fresh start to carry out.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_leaked_lock_of_its_own_still_comes_back() {
        use zeron_update::transaction::{self, Phase};
        let install = MacInstall::new();
        install.request_update();
        let leaked = InstanceLock::acquire(&install.data).unwrap();
        assert_eq!(after_a_failed_update(&install.data), AfterFailedUpdate::Exec);
        assert_eq!(transaction::read(&install.data).unwrap().phase, Phase::Requested);
        drop(leaked);
    }

    /// A trial or restore under way belongs to another process (its
    /// watchdog's to settle): the data directory free or not, it isn't
    /// overwritten and this engine doesn't come back over it.
    #[cfg(target_os = "macos")]
    #[test]
    fn another_process_trial_is_left_alone() {
        use zeron_update::transaction::{self, Phase};
        let install = MacInstall::new();
        install.request_update();
        for phase in [Phase::Trial, Phase::Restoring] {
            let mut record = transaction::read(&install.data).unwrap();
            record.phase = phase;
            record.trial_pid = Some(1);
            std::fs::write(
                install.data.join("update-transaction.json"),
                serde_json::to_vec(&record).unwrap(),
            )
            .unwrap();
            assert_eq!(after_a_failed_update(&install.data), AfterFailedUpdate::LeaveIt);
            assert_eq!(transaction::read(&install.data).unwrap().phase, phase);
        }
    }

    /// Second review of #1345: the app installed a newer version itself
    /// while an update was recorded. Following it (`SupersededRestart`, which
    /// doesn't try a version twice) must stop the engine, unflagged — the app
    /// starts the new one and its boot settles the record.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_newer_app_installed_during_an_update_still_stops_the_engine() {
        let install = MacInstall::new();
        install.request_update();
        std::fs::write(
            install.bundle.join("Contents/Info.plist"),
            "<plist><key>ZeronVersion</key><string>3.0.0</string></plist>",
        )
        .unwrap();
        assert_eq!(install.restart_from(&install.engine), (true, true, false));
    }

    /// A signal or a sign-out that wins the race against the update's own
    /// stop is a stop: the engine doesn't bring itself back.
    #[cfg(target_os = "macos")]
    #[test]
    fn only_the_updates_own_stop_brings_it_back() {
        assert!(comes_back(true, true));
        assert!(!comes_back(true, false), "SIGTERM or sign-out won");
        assert!(!comes_back(false, true), "StopEngine, or the app's own update");
        assert!(!comes_back(false, false));
    }

    /// The app is found by where it runs from — its bundle's `Contents/MacOS`
    /// — and never mistaken for the engine's own executables there.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_running_app_is_found_in_its_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let macos = dir.path().join("Bundle").join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        // A program of our own (copies of system binaries are killed on
        // launch): it just waits.
        let source = dir.path().join("wait.c");
        std::fs::write(
            &source,
            "#include <unistd.h>\nint main(void){sleep(30);return 0;}\n",
        )
        .unwrap();
        let built = dir.path().join("wait");
        let compiled = std::process::Command::new("cc")
            .arg("-o")
            .arg(&built)
            .arg(&source)
            .status();
        if !compiled.is_ok_and(|status| status.success()) {
            eprintln!("no C compiler: skipping");
            return;
        }
        let start = |name: &str| {
            let exe = macos.join(name);
            std::fs::copy(&built, &exe).unwrap();
            std::process::Command::new(&exe).spawn().unwrap()
        };
        assert!(!app_running(&macos), "nothing runs from the bundle yet");
        let mut engine = start("Zeron Engine");
        std::thread::sleep(Duration::from_millis(200));
        assert!(!app_running(&macos), "the engine isn't the app");
        let mut app = start("zeron");
        std::thread::sleep(Duration::from_millis(200));
        assert!(app_running(&macos));
        for child in [&mut engine, &mut app] {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
