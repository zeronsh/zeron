//! A Windows install the old Zeron's updater left as this `zeron.exe` alone.
//!
//! Zeron before this app was one self-contained `zeron.exe`, and its updater
//! (still running on every PC that hasn't updated) installs a release by
//! swapping that one file (`zeron-<version>-windows-<arch>.exe`). This app
//! also needs its version's folder beside it (`beta\<version>\`: scripts,
//! plugins, the engine). So before anything else, a `zeron.exe` that finds
//! its folder missing brings it in from the release feed, as an update would
//! (crates/update `windows::complete_install`), whether it was started as the
//! app, relaunched by the update (`--wait-for-exit`), or started as
//! `zeron.exe engine` by the old engine restarting into the new version, or
//! as an update's trial, run from where the update staged it in place of the
//! installation (`ZERON_INSTALLED_AT`, as an engine on another device asked).
//!
//! On failure nothing has changed. A window launch says so and offers to try
//! again; any other command prints why and exits, and the next start retries.

use std::path::Path;

use zeron_update::windows::{self, Completion};

use crate::about::APP_VERSION;

/// Complete this installation if it needs it; returns once this process may
/// go on. Exits instead when the newer `zeron.exe` the feed had by then ran
/// in its place, or when the person gives up.
pub fn complete_if_needed(argv: &[String]) {
    windows::set_packaged();
    // Answered without the folder: the old updater checks a staged copy so.
    if matches!(argv.get(1).map(String::as_str), Some("--version" | "-V" | "--help" | "-h" | "help")) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(directory) = windows::incomplete_install(&exe, APP_VERSION) else {
        return;
    };
    // A window launch: nothing, a link, or the update's relaunch.
    let window = argv
        .get(1)
        .is_none_or(|arg| arg.starts_with("--") || arg.starts_with("zeron:"));
    loop {
        match complete(&directory) {
            Ok(Completion::Complete | Completion::Installed) => return,
            Ok(Completion::Updated(version)) => {
                run_installed(&directory, &argv[1..], &version);
            }
            Err(error) => {
                eprintln!("Error: Zeron couldn't finish installing this update: {error}");
                if !window || !ask_to_retry(&error) {
                    std::process::exit(1);
                }
            }
        }
    }
}

/// The error in words, its causes included.
fn complete(directory: &Path) -> Result<Completion, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime
        .block_on(windows::complete_install(zeron_cli::DEFAULT_EDGE_URL, directory, APP_VERSION))
        .map_err(|error| format!("{error:#}"))
}

/// The feed had a newer release, now installed as `zeron.exe`: it runs this
/// launch instead, and its exit is this one's.
fn run_installed(directory: &Path, args: &[String], version: &str) -> ! {
    let installed = directory.join("zeron.exe");
    match std::process::Command::new(&installed).args(args).status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("Error: Zeron {version} is installed, but couldn't start: {error}");
            std::process::exit(1);
        }
    }
}

/// Retry or Cancel, in a dialog of the system's own (this app's window
/// can't open without the files it's missing).
fn ask_to_retry(error: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDRETRY, MB_ICONERROR, MB_RETRYCANCEL, MB_SETFOREGROUND, MessageBoxW,
    };
    let text = format!(
        "Zeron couldn't finish installing its update.\n\n{error}\n\nCheck your internet connection and choose Retry. Nothing was removed: if you cancel, Zeron tries again the next time it opens."
    );
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (text, caption) = (wide(&text), wide("Zeron"));
    // SAFETY: both are NUL-terminated UTF-16 strings that outlive the call.
    let answer = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_RETRYCANCEL | MB_ICONERROR | MB_SETFOREGROUND,
        )
    };
    answer == IDRETRY
}
