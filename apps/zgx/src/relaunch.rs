//! Reload Zeron (the palette's, `app.relaunch`): the app quits and starts
//! again, a fresh process on the same command line, in the same folder and
//! environment as this launch had (a production run's variables,
//! `main --inspect`). The engine isn't restarted: it runs detached, and the
//! new app connects to it as any launch does; the last chat viewed opens
//! again as it does on any launch.
//!
//! The windows close first (each saves its `ui.persist` state, devtools let
//! go of their socket, the Zeron app gives up its application ID), then the
//! new process starts, detached in a session of its own so it outlives this
//! one, and this one exits. The new one is told this one's pid
//! (`ZERON_ZGX_RELAUNCHED_FROM`) and waits for it to be gone before it
//! starts, so it is never a second instance beside a dying first.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The pid of the launch this one replaces, which it waits out.
pub const PREDECESSOR: &str = "ZERON_ZGX_RELAUNCHED_FROM";

/// What a launch's own launcher handed it once, which a process started by
/// this one must not reuse: XDG activation and X11 startup-notification
/// tokens (spent on the first window), and the predecessor this launch
/// waited for.
const NOT_PASSED_ON: [&str; 3] = ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID", PREDECESSOR];

/// How long a relaunch waits for the launch it replaces to exit.
const WAIT_AT_MOST: Duration = Duration::from_secs(10);

/// This launch as it started: its arguments (the program's own first), its
/// environment and its working folder.
#[derive(Clone, Debug)]
pub struct Launch {
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub cwd: Option<PathBuf>,
}

static LAUNCH: OnceLock<Launch> = OnceLock::new();
static ASKED: AtomicBool = AtomicBool::new(false);

/// First in `main`, before anything sets a variable: this launch's command
/// line, environment and folder kept for a relaunch; and, started by one,
/// the launch it replaces waited out.
pub fn remember() {
    let launch = Launch {
        args: std::env::args_os().collect(),
        env: std::env::vars_os().collect(),
        cwd: std::env::current_dir().ok(),
    };
    let predecessor = launch
        .env
        .iter()
        .find(|(key, _)| key == PREDECESSOR)
        .and_then(|(_, pid)| pid.to_str()?.parse::<u32>().ok());
    let _ = LAUNCH.set(launch);
    if std::env::var_os(PREDECESSOR).is_some() {
        // SAFETY: first thing in main, before any thread is started.
        unsafe { std::env::remove_var(PREDECESSOR) };
    }
    if let Some(pid) = predecessor {
        wait_for_exit(pid, WAIT_AT_MOST);
    }
}

/// Asks for a relaunch: `quit` ends the app (its windows close and save),
/// and `main` starts the new one ([`after_quit`]).
pub fn ask(quit: impl FnOnce()) {
    ASKED.store(true, Ordering::SeqCst);
    quit();
}

/// Last in `main`, once the app has quit: the new process, if a relaunch
/// was asked for.
pub fn after_quit() {
    if !ASKED.load(Ordering::SeqCst) {
        return;
    }
    let Some(launch) = LAUNCH.get() else {
        eprintln!("zeron-zgx: relaunch: this launch wasn't remembered");
        return;
    };
    let exe = program(std::env::current_exe().ok(), launch);
    let mut command = command(launch, &exe, std::process::id());
    detach(&mut command);
    match command.spawn() {
        Ok(child) => eprintln!(
            "zeron-zgx: relaunched as {} ({})",
            child.id(),
            exe.display()
        ),
        Err(error) => eprintln!("zeron-zgx: relaunch of {} failed: {error}", exe.display()),
    }
}

/// The new process's command: `exe` with this launch's arguments, in its
/// folder, with its environment but for what isn't passed on, told `pid`
/// to wait for.
pub fn command(launch: &Launch, exe: &Path, pid: u32) -> Command {
    let mut command = Command::new(exe);
    // (Its first word as this launch's was, so it reads the same in `ps`;
    // the program run is still `exe`.)
    #[cfg(unix)]
    if let Some(first) = launch.args.first() {
        use std::os::unix::process::CommandExt;
        command.arg0(first);
    }
    command.args(launch.args.iter().skip(1));
    command.env_clear();
    command.envs(
        launch
            .env
            .iter()
            .filter(|(key, _)| !NOT_PASSED_ON.iter().any(|skip| key == OsStr::new(skip)))
            .map(|(key, value)| (key, value)),
    );
    command.env(PREDECESSOR, pid.to_string());
    if let Some(cwd) = &launch.cwd {
        command.current_dir(cwd);
    }
    command
}

/// The executable to start: this one's, or, replaced on disk since it
/// started (a rebuild; Linux then names it `<path> (deleted)`), what is at
/// its path now; else the command line's first word, from the launch's
/// folder.
pub fn program(current: Option<PathBuf>, launch: &Launch) -> PathBuf {
    if let Some(exe) = &current {
        if exe.is_file() {
            return exe.clone();
        }
        if let Some(path) = exe.to_str().and_then(|p| p.strip_suffix(" (deleted)"))
            && Path::new(path).is_file()
        {
            return PathBuf::from(path);
        }
    }
    match launch.args.first().map(PathBuf::from) {
        // (A bare name is looked up on PATH, as it was.)
        Some(first) if first.is_relative() && first.components().count() > 1 => match &launch.cwd {
            Some(cwd) => cwd.join(first),
            None => first,
        },
        Some(first) => first,
        None => current.unwrap_or_default(),
    }
}

/// The new process in a session (Unix) or process group (Windows) of its
/// own, so this one's exit, or its terminal's, doesn't end it. Its output
/// goes where this one's went (a log file, the terminal).
fn detach(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid` is async-signal-safe, and nothing else runs
        // between fork and exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }
}

/// Waits until `pid` has exited, or `at_most` has passed.
fn wait_for_exit(pid: u32, at_most: Duration) {
    let start = Instant::now();
    while alive(pid) {
        if start.elapsed() >= at_most {
            eprintln!("zeron-zgx: relaunch: {pid} is still running; starting anyway");
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Whether `pid` still runs: not gone, and not a zombie waiting to be
/// reaped (exited, as far as an app instance goes).
fn alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        // `pid (comm) state …`: the state after the last parenthesis.
        let state = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.trim_start().chars().next());
        !matches!(state, Some('Z' | 'X') | None)
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: signal 0 only checks that the process exists.
        unsafe {
            libc::kill(pid, 0) == 0
                || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
        }
    }
    #[cfg(not(unix))]
    {
        // (Windows: the old process has closed its windows before the new
        // one starts; nothing is waited for.)
        let _ = pid;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> Launch {
        let pairs = [
            ("ZERON_ZGX_ENV", "production"),
            ("ZERON_ZGX_ENGINE_PORT", "27881"),
            ("XDG_ACTIVATION_TOKEN", "spent"),
            ("DESKTOP_STARTUP_ID", "spent"),
            (PREDECESSOR, "7"),
        ];
        Launch {
            args: ["./target/release/zeron-zgx", "main", "--inspect"]
                .map(OsString::from)
                .to_vec(),
            env: pairs
                .iter()
                .map(|(k, v)| (OsString::from(k), OsString::from(v)))
                .collect(),
            cwd: Some(PathBuf::from("/work/apps/zgx")),
        }
    }

    fn envs(command: &Command) -> Vec<(String, Option<String>)> {
        command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    #[test]
    fn the_same_command_line_folder_and_environment() {
        let exe = PathBuf::from("/work/apps/zgx/target/release/zeron-zgx");
        let command = command(&launch(), &exe, 4242);
        assert_eq!(command.get_program(), exe.as_os_str());
        let args: Vec<&OsStr> = command.get_args().collect();
        assert_eq!(args, ["main", "--inspect"]);
        assert_eq!(command.get_current_dir(), Some(Path::new("/work/apps/zgx")));
        let envs = envs(&command);
        let get = |key: &str| envs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        assert_eq!(get("ZERON_ZGX_ENV"), Some(Some("production".into())));
        assert_eq!(get("ZERON_ZGX_ENGINE_PORT"), Some(Some("27881".into())));
        // Told this process, not the one this launch replaced.
        assert_eq!(get(PREDECESSOR), Some(Some("4242".into())));
        assert_eq!(envs.iter().filter(|(k, _)| k == PREDECESSOR).count(), 1);
        // A launcher's one-time tokens aren't passed on.
        assert_eq!(get("XDG_ACTIVATION_TOKEN"), None);
        assert_eq!(get("DESKTOP_STARTUP_ID"), None);
    }

    #[test]
    fn a_bare_launch_has_no_arguments() {
        let mut bare = launch();
        bare.args.truncate(1);
        let command = command(&bare, Path::new("/bin/zeron-zgx"), 1);
        assert_eq!(command.get_args().count(), 0);
    }

    #[test]
    fn an_executable_rebuilt_since_launch_is_the_one_at_its_path() {
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let deleted = PathBuf::from(format!("{} (deleted)", here.display()));
        assert_eq!(program(Some(deleted), &launch()), here);
        assert_eq!(program(Some(here.clone()), &launch()), here);
    }

    #[test]
    fn without_one_the_command_lines_first_word_from_its_folder() {
        let gone = PathBuf::from("/nowhere/zeron-zgx (deleted)");
        assert_eq!(
            program(Some(gone), &launch()),
            PathBuf::from("/work/apps/zgx/./target/release/zeron-zgx")
        );
        assert_eq!(
            program(None, &launch()),
            PathBuf::from("/work/apps/zgx/./target/release/zeron-zgx")
        );
    }

    #[test]
    fn this_process_is_alive_and_a_gone_one_is_not() {
        assert!(alive(std::process::id()) || cfg!(not(unix)));
        // (Above any pid_max Linux allows.)
        assert!(!alive(4_194_305 + 1000));
        let start = Instant::now();
        wait_for_exit(4_194_305 + 1000, Duration::from_secs(5));
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
