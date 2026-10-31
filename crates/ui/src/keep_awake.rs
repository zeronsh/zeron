//! Keep the Mac awake while an agent is working, lid closed included.
//!
//! Power assertions (`caffeinate`, `IOPMAssertion`) stop idle sleep but not
//! the clamshell sleep a closed lid triggers. Only `pmset disablesleep 1`
//! does that, and it needs root. Zeron runs it through `sudo -n` against a
//! narrow sudoers rule (`/usr/bin/pmset disablesleep 0|1` only); when the
//! rule is missing, switching the setting on installs it behind the standard
//! macOS administrator prompt.
//!
//! Sleep is held only while some session is `Working` (the shell calls
//! [`sync`] on every state change), and released [`GRACE`] after the last one
//! stops so the gap between two turns doesn't flap it. If the lid is already
//! closed when the work ends, the Mac is put to sleep right away.
//!
//! A detached watchdog restores sleep once the Zeron process is gone, so a
//! crash never leaves the Mac sleepless. If sleep was already disabled by
//! someone else (another clamshell tool), Zeron never touches it.
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const PMSET: &str = "/usr/bin/pmset";
const SUDOERS_FILE: &str = "/etc/sudoers.d/zeron-keep-awake";
/// Between turns an agent goes idle for a moment; don't let the Mac doze off
/// (or the lid-closed sleep kick in) in that gap.
pub const GRACE: Duration = Duration::from_secs(20);

/// The setting.
static ENABLED: AtomicBool = AtomicBool::new(false);
/// Whether Zeron itself turned sleep off (and so must turn it back on).
static OWNED: AtomicBool = AtomicBool::new(false);
/// Whether work is running right now (as of the last [`sync`]).
static WORKING: AtomicBool = AtomicBool::new(false);
/// Serializes hold/release (each shells out).
static PMSET_LOCK: Mutex<()> = Mutex::new(());
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);
static BUSY: AtomicBool = AtomicBool::new(false);
static RELEASE_PENDING: AtomicBool = AtomicBool::new(false);

/// The last failure, shown under the setting. `None` after a success.
pub fn last_error() -> Option<String> {
    LAST_ERROR.lock().ok().and_then(|error| error.clone())
}

fn set_error(error: Option<String>) {
    if let Ok(mut slot) = LAST_ERROR.lock() {
        *slot = error;
    }
}

/// The settings switch is applying (the admin prompt may be up).
pub fn busy() -> bool {
    BUSY.load(Ordering::Acquire)
}

/// Whether Zeron holds sleep off right now.
pub fn holding() -> bool {
    OWNED.load(Ordering::Acquire)
}

/// Launch: adopt the saved preference without prompting. Nothing is held
/// until work starts.
pub fn init(enabled: bool) {
    ENABLED.store(enabled, Ordering::Release);
}

/// The settings switch. Turning it on makes sure the sudoers rule exists
/// (prompting once if needed) off the UI thread; turning it off releases at
/// once. The outcome lands in [`last_error`].
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Release);
    if !cfg!(target_os = "macos") {
        return;
    }
    BUSY.store(true, Ordering::Release);
    std::thread::spawn(move || {
        let result = if enabled {
            prepare().map(|()| {
                if WORKING.load(Ordering::Acquire) {
                    let _ = hold();
                }
            })
        } else {
            release(false)
        };
        set_error(result.err());
        BUSY.store(false, Ordering::Release);
    });
}

/// The shell reports whether any agent is working. Starts holding at once;
/// stops [`GRACE`] after the last work ends (unless it resumes).
pub fn sync(working: bool, cx: &mut gpui::App) {
    let was = WORKING.swap(working, Ordering::AcqRel);
    if !cfg!(target_os = "macos") || !ENABLED.load(Ordering::Acquire) {
        return;
    }
    if working {
        RELEASE_PENDING.store(false, Ordering::Release);
        if !OWNED.load(Ordering::Acquire) {
            std::thread::spawn(|| {
                if let Err(error) = hold() {
                    set_error(Some(error));
                }
            });
        }
    } else if was && OWNED.load(Ordering::Acquire) {
        RELEASE_PENDING.store(true, Ordering::Release);
        cx.spawn(async move |cx| {
            cx.background_executor().timer(GRACE).await;
            if RELEASE_PENDING.swap(false, Ordering::AcqRel) && !WORKING.load(Ordering::Acquire) {
                std::thread::spawn(|| {
                    let _ = release(true);
                });
            }
        })
        .detach();
    }
}

/// `SleepDisabled` from `pmset -g`: whether sleep is off right now.
fn sleep_disabled() -> bool {
    Command::new(PMSET)
        .arg("-g")
        .output()
        .ok()
        .map(|out| parse_sleep_disabled(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or(false)
}

/// Pure: read `SleepDisabled` out of `pmset -g` output.
pub fn parse_sleep_disabled(output: &str) -> bool {
    output.lines().any(|line| {
        let mut parts = line.split_whitespace();
        parts.next() == Some("SleepDisabled") && parts.next() == Some("1")
    })
}

/// Pure: whether `ioreg -r -k AppleClamshellState` reports the lid closed.
pub fn parse_lid_closed(output: &str) -> bool {
    output
        .lines()
        .any(|line| line.contains("\"AppleClamshellState\" = Yes"))
}

fn lid_closed() -> bool {
    Command::new("/usr/sbin/ioreg")
        .args(["-r", "-k", "AppleClamshellState", "-d", "1"])
        .output()
        .ok()
        .is_some_and(|out| parse_lid_closed(&String::from_utf8_lossy(&out.stdout)))
}

fn sudo_pmset(args: &[&str]) -> Result<(), String> {
    let status = Command::new("/usr/bin/sudo")
        .arg("-n")
        .arg(PMSET)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("sudo refused pmset".into())
    }
}

/// Whether sudo lets this user run the rule's commands without a password.
/// `sudo -n -l <cmd>` only checks; it runs nothing.
fn rule_allows() -> bool {
    Command::new("/usr/bin/sudo")
        .args(["-n", "-l", PMSET, "disablesleep", "1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The sudoers rule: this user may run exactly the two pmset commands.
pub fn sudoers_rule(user: &str) -> String {
    format!("{user} ALL=(root) NOPASSWD: {PMSET} disablesleep 1, {PMSET} disablesleep 0\n")
}

/// Make sure the rule exists, installing it behind the macOS admin prompt.
/// The rule is validated with `visudo -c` before it goes live, so a bad file
/// can never break sudo.
fn prepare() -> Result<(), String> {
    if rule_allows() {
        return Ok(());
    }
    let user = std::env::var("USER").map_err(|_| "unknown user".to_string())?;
    if user.is_empty()
        || !user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err("unsupported user name".into());
    }
    let staging = std::env::temp_dir().join(format!("zeron-keep-awake-{}", std::process::id()));
    std::fs::write(&staging, sudoers_rule(&user)).map_err(|error| error.to_string())?;
    let staging = staging.to_string_lossy().into_owned();
    if staging.contains('\'') || staging.contains('"') || staging.contains('\\') {
        return Err("unsupported temp path".into());
    }
    let script = format!(
        "/usr/sbin/visudo -cf '{staging}' && /usr/bin/install -m 0440 -o root -g wheel '{staging}' {SUDOERS_FILE}"
    );
    let apple = format!(
        "do shell script \"{script}\" with prompt \"Zeron needs permission to keep your Mac awake while agents work.\" with administrator privileges"
    );
    let output = Command::new("/usr/bin/osascript")
        .args(["-e", &apple])
        .output()
        .map_err(|error| error.to_string())?;
    let _ = std::fs::remove_file(&staging);
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("-128") {
            Err("Permission was not granted.".into())
        } else {
            Err(format!("Could not set up keep-awake: {}", stderr.trim()))
        }
    }
}

fn hold() -> Result<(), String> {
    let _guard = PMSET_LOCK.lock().map_err(|_| "lock poisoned".to_string())?;
    if OWNED.load(Ordering::Acquire) || !ENABLED.load(Ordering::Acquire) {
        return Ok(());
    }
    if sleep_disabled() {
        // Someone else already holds it; never undo a choice that isn't ours.
        return Ok(());
    }
    sudo_pmset(&["disablesleep", "1"]).map_err(|_| "Could not disable sleep.".to_string())?;
    OWNED.store(true, Ordering::Release);
    spawn_watchdog();
    Ok(())
}

/// Give sleep back. `sleep_if_lid_closed`: the work just ended, and a lid
/// closed meanwhile should now put the Mac to sleep as it would have.
fn release(sleep_if_lid_closed: bool) -> Result<(), String> {
    let _guard = PMSET_LOCK.lock().map_err(|_| "lock poisoned".to_string())?;
    if !OWNED.swap(false, Ordering::AcqRel) {
        return Ok(());
    }
    if sudo_pmset(&["disablesleep", "0"]).is_err() {
        OWNED.store(true, Ordering::Release);
        return Err("Could not restore sleep.".into());
    }
    let _ = std::fs::remove_file(marker_path());
    if sleep_if_lid_closed && lid_closed() {
        let _ = Command::new(PMSET)
            .arg("sleepnow")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    Ok(())
}

/// Restore sleep when this process exits, however it exits. One watchdog per
/// process: it outlives Zeron (reparented to launchd) and only restores sleep
/// if Zeron still held it.
fn spawn_watchdog() {
    static SPAWNED: AtomicBool = AtomicBool::new(false);
    let marker = marker_path();
    let _ = std::fs::write(&marker, b"");
    if SPAWNED.swap(true, Ordering::AcqRel) {
        return;
    }
    let pid = std::process::id();
    let marker = marker.to_string_lossy().into_owned();
    if marker.contains('\'') {
        return;
    }
    let script = format!(
        "while /bin/kill -0 {pid} 2>/dev/null; do /bin/sleep 5; done; \
         if [ -e '{marker}' ]; then /bin/rm -f '{marker}'; /usr/bin/sudo -n {PMSET} disablesleep 0; fi"
    );
    let _ = Command::new("/bin/sh")
        .args(["-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Present while Zeron holds sleep off; the watchdog acts only on it.
fn marker_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("zeron-keep-awake-held-{}", std::process::id()))
}

/// Restore sleep on a clean quit (the watchdog covers everything else).
pub fn shutdown() {
    let _ = release(false);
    let _ = std::fs::remove_file(marker_path());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sleep_disabled_from_pmset_output() {
        assert!(parse_sleep_disabled(" SleepDisabled\t\t1\n displaysleep 10\n"));
        assert!(!parse_sleep_disabled(" SleepDisabled\t\t0\n"));
        assert!(!parse_sleep_disabled(" sleep 1\n"));
    }

    #[test]
    fn reads_the_lid_state_from_ioreg() {
        assert!(parse_lid_closed("  | \"AppleClamshellState\" = Yes\n"));
        assert!(!parse_lid_closed("  | \"AppleClamshellState\" = No\n"));
        assert!(!parse_lid_closed(""));
    }

    #[test]
    fn sudoers_rule_names_only_the_two_pmset_commands() {
        assert_eq!(
            sudoers_rule("alice"),
            "alice ALL=(root) NOPASSWD: /usr/bin/pmset disablesleep 1, /usr/bin/pmset disablesleep 0\n"
        );
    }
}
