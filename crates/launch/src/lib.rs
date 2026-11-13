//! How a Zeron app starts its engine: the engine executable beside the app's
//! ("Zeron Engine" in a macOS bundle, `zeron-engine` elsewhere), else the app
//! binary's [`ENGINE_SUBCOMMAND`]; the environment it's given; detached, so
//! it outlives the app. The engine's `managed` module, the command line
//! (`zeron headless` and the other engine subcommands, run in the helper) and
//! the zgx app all start it through here, without linking the engine.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// The app binary's subcommand that runs the background engine, for
/// installs without a separate engine executable (Windows; development runs
/// that only built the app).
pub const ENGINE_SUBCOMMAND: &str = "engine";

/// The separate engine executable's name next to the app's.
#[cfg(target_os = "macos")]
pub const ENGINE_EXECUTABLES: &[&str] = &["Zeron Engine", "zeron-engine"];
#[cfg(windows)]
pub const ENGINE_EXECUTABLES: &[&str] = &["zeron-engine.exe"];
#[cfg(not(any(target_os = "macos", windows)))]
pub const ENGINE_EXECUTABLES: &[&str] = &["zeron-engine"];

/// Set on the background engine's environment by the app that started it.
pub const MANAGED_ENV: &str = "ZERON_MANAGED_ENGINE";
/// The app to open for a clicked banner: its bundle on macOS, its
/// executable elsewhere.
pub const APP_ENV: &str = "ZERON_APP";

static FULL_APP_ENGINE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// This app's engine is another release's whole app binary, which runs the
/// engine as its [`ENGINE_SUBCOMMAND`] (a Windows release has no separate
/// engine executable): the zgx beta on Windows, which carries stable's. Set
/// once, before anything asks. Nothing else sets it, so other builds resolve
/// their engine as ever.
pub fn set_full_app_engine(binary: PathBuf) {
    let _ = FULL_APP_ENGINE.set(binary);
}

fn full_app_engine() -> Option<&'static Path> {
    FULL_APP_ENGINE.get().map(PathBuf::as_path).filter(|path| path.is_file())
}

/// The engine executable for the app at `app_exe`, if there is one: beside it,
/// or the one [`set_full_app_engine`] named.
pub fn engine_executable(app_exe: &Path) -> Option<PathBuf> {
    if let Some(engine) = full_app_engine() {
        return Some(engine.to_path_buf());
    }
    let dir = app_exe.parent().unwrap_or(Path::new("."));
    ENGINE_EXECUTABLES
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

/// The command that runs the background engine for the app at `app_exe`:
/// the engine executable beside it, else the app binary's
/// [`ENGINE_SUBCOMMAND`].
pub fn engine_command(app_exe: &Path) -> Command {
    if let Some(engine) = full_app_engine() {
        let mut command = Command::new(engine);
        command.arg(ENGINE_SUBCOMMAND);
        return command;
    }
    match engine_executable(app_exe) {
        Some(engine) => Command::new(engine),
        None => {
            let mut command = Command::new(app_exe);
            command.arg(ENGINE_SUBCOMMAND);
            command
        }
    }
}

/// What a clicked banner opens: the app bundle on macOS (the executable's
/// enclosing `.app`), the executable elsewhere.
pub fn app_path(app_exe: &Path) -> PathBuf {
    app_exe
        .ancestors()
        .find(|dir| dir.extension().is_some_and(|ext| ext == "app"))
        .unwrap_or(app_exe)
        .to_path_buf()
}

/// What the background engine is started with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchConfig {
    pub data_dir: PathBuf,
    pub ipc_port: u16,
    pub edge_url: String,
    /// `None` selects development auth.
    pub workos_client_id: Option<String>,
    pub edge_token: Option<String>,
    pub org_id: Option<String>,
}

/// Start the background engine for `config`, detached from the app: its
/// own session/process group, no inherited console or pipes, so the app
/// exiting (or its terminal closing) leaves it running.
pub fn launch(config: &LaunchConfig, app_exe: &Path) -> std::io::Result<Child> {
    let mut command = engine_command(app_exe);
    command
        .env(MANAGED_ENV, "1")
        .env(APP_ENV, app_path(app_exe))
        .env("ZERON_DATA_DIR", &config.data_dir)
        .env("ZERON_IPC_PORT", config.ipc_port.to_string())
        .env("ZERON_EDGE_URL", &config.edge_url)
        // Empty selects development auth, exactly as `None` does in-process.
        .env(
            "ZERON_WORKOS_CLIENT_ID",
            config.workos_client_id.as_deref().unwrap_or(""),
        );
    match &config.edge_token {
        Some(token) => command.env("ZERON_EDGE_TOKEN", token),
        None => command.env_remove("ZERON_EDGE_TOKEN"),
    };
    if let Some(org) = &config.org_id {
        command.env("ZERON_ORG_ID", org);
    }
    spawn_detached(&mut command)
}

/// Spawn `command` so it survives its parent.
pub fn spawn_detached(command: &mut Command) -> std::io::Result<Child> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // A session of its own: no controlling terminal to hang up on it.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command.spawn()
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        use windows_sys::Win32::System::Threading::{
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
        };
        // Leave the app's job object too (a launcher's kill-on-close job
        // would take the engine down with the app) — when the job allows it.
        let detached = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
        command.creation_flags(detached | CREATE_BREAKAWAY_FROM_JOB);
        match command.spawn() {
            Ok(child) => Ok(child),
            Err(_) => {
                command.creation_flags(detached);
                command.spawn()
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    command.spawn()
}

/// Whether an engine holds `data_dir` (its `engine.lock`, crates/engine's
/// `InstanceLock`): one starting up, or one still shutting down.
///
/// Asked by trying it, and letting go at once. An engine that finds it taken
/// for that moment tries again.
pub fn data_dir_held(data_dir: &Path) -> bool {
    let Ok(lock) = std::fs::File::options()
        .read(true)
        .write(true)
        .open(data_dir.join("engine.lock"))
    else {
        return false;
    };
    matches!(lock.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

/// Reap `child` whenever it exits, so a background engine that stops while
/// the app runs never lingers as a zombie.
pub fn reap(mut child: Child) {
    let _ = std::thread::Builder::new()
        .name("engine-reaper".into())
        .spawn(move || {
            let _ = child.wait();
        });
}

/// Become the engine helper beside the app at `app_exe`, given `args` (this
/// process's own, `zeron headless`'s `headless` and on): the standard helper
/// pattern for an app whose interface binary doesn't link the engine. On
/// Unix the helper replaces this process (`exec`), so the pid, signals,
/// standard streams and exit status are the helper's own — a service
/// manager watching `zeron headless` watches the engine. Elsewhere it runs
/// as a child with the same streams, and its exit status is passed on.
/// Returns only when it couldn't start.
pub fn exec_helper(app_exe: &Path, args: impl IntoIterator<Item = OsString>) -> std::io::Error {
    let Some(helper) = engine_executable(app_exe) else {
        return std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "no engine beside {} ({})",
                app_exe.display(),
                ENGINE_EXECUTABLES.join(" or ")
            ),
        );
    };
    let mut command = Command::new(&helper);
    command.args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.exec()
    }
    #[cfg(not(unix))]
    {
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => return error,
        };
        // Only for an update's trial (`ZERON_INSTALLED_AT`, crates/update):
        // otherwise this one must outlive the helper's own clean shutdown
        // (Ctrl+C reaches both, and a job would cut the helper's short).
        #[cfg(windows)]
        if std::env::var_os("ZERON_INSTALLED_AT").is_some() {
            ends_with_this_process(&child);
        }
        match child.wait() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(error) => error,
        }
    }
}

/// Windows can't replace a process, so this one waits on the helper in its
/// place. In an update's trial, whatever stops this one must stop the helper
/// too: the watchdog ends a failed trial by terminating the process it
/// started, which is this one, and the helper (the engine) must not outlive
/// it to hold the data the old version is restoring. A job that ends with this process does
/// it, however it ends. What the helper starts in turn (agents, its own
/// successor) leaves the job. Best effort: without a job, it's as before.
#[cfg(windows)]
fn ends_with_this_process(child: &std::process::Child) {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    // SAFETY: plain Win32 calls on a job handle created here and the child's
    // live process handle; the information struct is zeroed and sized.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return;
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK;
        let set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if set == 0 || AssignProcessToJobObject(job, child.as_raw_handle()) == 0 {
            CloseHandle(job);
        }
        // Otherwise the handle stays open for this process's life: closing it,
        // at its end, is what ends the helper.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_command_prefers_the_engine_beside_the_app() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("zeron");
        std::fs::write(&app, "").unwrap();
        // Only the app: it runs the engine as a subcommand.
        let command = engine_command(&app);
        assert_eq!(command.get_program(), app.as_os_str());
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![std::ffi::OsStr::new(ENGINE_SUBCOMMAND)]
        );
        assert_eq!(engine_executable(&app), None);
        // With the engine executable beside it, that runs instead.
        let engine = dir.path().join(ENGINE_EXECUTABLES[0]);
        std::fs::write(&engine, "").unwrap();
        let command = engine_command(&app);
        assert_eq!(command.get_program(), engine.as_os_str());
        assert_eq!(command.get_args().count(), 0);
        assert_eq!(engine_executable(&app), Some(engine));
    }

    #[test]
    fn app_path_is_the_bundle_on_macos_layouts() {
        assert_eq!(
            app_path(Path::new("/Applications/Zeron.app/Contents/MacOS/zeron")),
            PathBuf::from("/Applications/Zeron.app")
        );
        assert_eq!(
            app_path(Path::new("/home/me/.zeron/app/current/zeron")),
            PathBuf::from("/home/me/.zeron/app/current/zeron")
        );
    }

    #[test]
    fn a_data_dir_is_held_while_its_lock_is() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!data_dir_held(dir.path()), "no lock file: nobody holds it");
        let lock = std::fs::File::create(dir.path().join("engine.lock")).unwrap();
        assert!(!data_dir_held(dir.path()), "a lock file nobody locks");
        lock.try_lock().unwrap();
        assert!(data_dir_held(dir.path()));
        lock.unlock().unwrap();
        assert!(!data_dir_held(dir.path()));
        // Asking lets go at once: the engine can take it after.
        lock.try_lock().unwrap();
    }

    #[test]
    fn a_missing_helper_is_an_error_not_a_hang() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("zeron");
        let error = exec_helper(&app, [OsString::from("headless")]);
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }
}
