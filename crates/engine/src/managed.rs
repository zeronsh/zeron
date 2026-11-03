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
//! itself.

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
        // macOS: the new version is only ever started by the app (see
        // `app_starts_engine`), so the restart waits for the app to run and
        // this engine keeps serving the old version until then.
        #[cfg(target_os = "macos")]
        zeron_update::set_managed_restart_ready(app_starts_engine);
        zeron_update::set_managed_restart(move || {
            if cfg!(target_os = "macos") {
                tracing::info!("restarting into the update: the app starts the new engine");
            } else {
                relaunch_self()?;
            }
            let _ = restart_stop.send(());
            Ok(())
        });
        let uninstalled_stop = stop_tx.clone();
        let boot = engine.spawn_boot(
            config.clone(),
            lock,
            Some(config.ipc_port),
            move |runtime| {
                if let Some(updater) = runtime.core().updater() {
                    updater.restart_when_superseded();
                }
                stop_when_uninstalled(runtime, uninstalled_stop);
            },
        );

        let mut auth_state = engine.auth.watch_state();
        let mut assembly = engine.state();
        let synced = engine.workspace_scope == WorkspaceScope::Synced;
        let outcome = tokio::select! {
            result = crate::shutdown_signal() => result.map_err(anyhow::Error::from),
            _ = stop_rx.recv() => {
                tracing::info!("background engine stop requested");
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
        outcome
    }
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
