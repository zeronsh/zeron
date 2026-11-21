//! keep the Mac awake while an agent is working, lid closed included.
//!
//! Power assertions (`caffeinate`, `IOPMAssertion`) stop idle sleep but not
//! the clamshell sleep a closed lid triggers. Only `pmset disablesleep 1`
//! does that, and it needs root. This is system-wide: Zeron, VPNs and other
//! apps keep running, not just the agent process. It also suppresses the
//! normal lid-close display blanking, so a lightweight lid/display monitor
//! requests `pmset displaysleepnow` while the built-in screen is still awake.
//! Display sleep needs no sudo and leaves system sleep disabled. An external
//! monitor is left alone (normal docked/clamshell use).
//!
//! Zeron runs the sleep setting through `sudo -n` against a
//! narrow sudoers rule (`/usr/bin/pmset disablesleep 0|1` only); when the
//! rule is missing, switching the setting on installs it behind the standard
//! macOS administrator prompt.
//!
//! Sleep is held only while a local session has live agent work, including
//! native subagents and background processes (the sessions host calls [`observe`] on
//! every state change), and released [`GRACE`] after the last one stops so the
//! gap between two turns doesn't flap it. If the lid is already
//! closed when the work ends, the Mac is put to sleep right away.
//!
//! A detached watchdog restores sleep once the Zeron process is gone, so a
//! crash never leaves the Mac sleepless. If sleep was already disabled by
//! someone else (another clamshell tool), Zeron never touches it.
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use zeron_proto::{Session, SessionStatus, view};
use zgx_script::{ScriptHost, Value};

/// Adopt the saved choice once; each window exposes the same runtime.
pub fn install(host: &ScriptHost) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if !STARTED.swap(true, Ordering::AcqRel) {
        let enabled = crate::engine::data_dir().is_some_and(|dir| dir.join(PREFERENCE).is_file());
        extern "C" fn on_exit() {
            let _ = std::panic::catch_unwind(shutdown);
        }
        unsafe {
            libc::atexit(on_exit);
        }
        init(enabled);
    }
    host.async_action("settings.awake.get", |_| async { Ok(state()) });
    host.async_action("settings.awake.set", |args| async move {
        crate::plugins::preview::refuse("settings.awake.set")?;
        let Some(Value::Bool(enabled)) = args.field("enabled") else {
            return Err("invalid".into());
        };
        if !cfg!(target_os = "macos") {
            return Err("Keep Mac awake is available on macOS only.".into());
        }
        let enabled = *enabled;
        let dir = crate::engine::data_dir().ok_or("offline")?;
        if BUSY.swap(true, Ordering::AcqRel) {
            return Err("busy".into());
        }
        let (tx, mut rx) = zgx::task::latest();
        std::thread::spawn(move || {
            let before = ENABLED.load(Ordering::Acquire);
            let result = set_enabled(enabled).and_then(|()| {
                let marker = dir.join(PREFERENCE);
                if enabled {
                    std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(marker, b""))
                } else {
                    std::fs::remove_file(marker).or_else(|e| {
                        if e.kind() == std::io::ErrorKind::NotFound {
                            Ok(())
                        } else {
                            Err(e)
                        }
                    })
                }
                .map_err(|e| e.to_string())
            });
            if result.is_err() {
                ENABLED.store(before, Ordering::Release);
                // Roll back without another administrator prompt.
                let _ = if before { hold() } else { release(None) };
            }
            set_error(result.err());
            BUSY.store(false, Ordering::Release);
            tx.send(());
        });
        rx.recv().await.ok_or("failed")?;
        Ok(state())
    });
}

fn state() -> Value {
    Value::record([
        ("enabled", Value::Bool(ENABLED.load(Ordering::Acquire))),
        ("supported", Value::Bool(cfg!(target_os = "macos"))),
        ("working", Value::Bool(WORKING.load(Ordering::Acquire))),
        ("holding", Value::Bool(holding())),
        ("busy", Value::Bool(busy())),
        (
            "error",
            last_error()
                .as_deref()
                .map(Value::from)
                .unwrap_or(Value::Nil),
        ),
    ])
}

/// Fresh work on this Mac, including idle parents with native children or
/// processes. Read ALL device rows, not the sidebar's winner for each chat:
/// a newer remote row must never hide work still running locally.
fn activity<'a>(
    local: Option<&str>,
    rows: impl IntoIterator<Item = &'a Session>,
    now: DateTime<Utc>,
) -> (bool, Option<i64>) {
    let Some(local) = local else {
        return (false, None);
    };
    let mut working = false;
    let mut expires = None;
    for s in rows {
        if s.device_id != local {
            continue;
        }
        let work = (s.activity_status() == SessionStatus::Working && !view::session_stale(s, now))
            || view::running_subagents(Some(s), now) > 0
            || !view::background_tasks(Some(s), now).is_empty()
            || view::waiting(Some(s), now).is_some();
        if work {
            working = true;
            let cutoff = s.updated_at.timestamp() + view::SESSION_STALE_MS / 1000 + 1;
            expires = Some(expires.map_or(cutoff, |old: i64| old.min(cutoff)));
        }
    }
    (working, expires)
}

/// Called from sessions::look_again, including heartbeats that don't change
/// anything drawn. Return the next work cutoff so an idle background task
/// expires even when the engine stops sending frames.
pub fn observe<'a>(
    local: Option<&str>,
    rows: impl IntoIterator<Item = &'a Session>,
    now: i64,
) -> Option<i64> {
    let at = DateTime::from_timestamp(now, 0).unwrap_or_default();
    let (working, expires) = activity(local, rows, at);
    sync(working);
    expires
}

const PREFERENCE: &str = "keep-awake-enabled";
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
static HOLD_PENDING: AtomicBool = AtomicBool::new(false);
/// Serializes hold/release (each shells out).
static PMSET_LOCK: Mutex<()> = Mutex::new(());
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);
static BUSY: AtomicBool = AtomicBool::new(false);
/// Invalidates older grace timers when work resumes or another turn ends.
static RELEASE_EPOCH: AtomicU64 = AtomicU64::new(0);
/// Number of in-flight grace release threads.
static RELEASE_TIMERS: AtomicUsize = AtomicUsize::new(0);
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

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
    SHUTTING_DOWN.store(false, Ordering::Release);
    ENABLED.store(enabled, Ordering::Release);
    if enabled {
        start_display_monitor();
    }
}

/// The settings switch. Turning it on makes sure the sudoers rule exists
/// (prompting once if needed) off the UI thread; turning it off releases at
/// once. The outcome lands in [`last_error`].
fn set_enabled(enabled: bool) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    start_display_monitor();
    if enabled {
        // An administrator prompt is allowed ONLY after a person switches
        // this on. Restoring a saved choice at launch never prompts.
        prepare()?;
        ENABLED.store(true, Ordering::Release);
        if let Err(error) = hold() {
            ENABLED.store(false, Ordering::Release);
            return Err(error);
        }
        Ok(())
    } else {
        ENABLED.store(false, Ordering::Release);
        RELEASE_EPOCH.fetch_add(1, Ordering::AcqRel);
        release(None)
    }
}

fn should_schedule_release(was_working: bool, owned: bool, active_timers: usize) -> bool {
    was_working || (owned && active_timers == 0)
}

/// Starts holding at once, and releases 20 seconds after the last work
/// ends. An unchanged heartbeat doesn't create another thread or timer.
fn sync(working: bool) {
    let was = WORKING.swap(working, Ordering::AcqRel);
    if !cfg!(target_os = "macos") || !ENABLED.load(Ordering::Acquire) {
        return;
    }
    if working {
        if !was {
            RELEASE_EPOCH.fetch_add(1, Ordering::AcqRel);
        }
        if !OWNED.load(Ordering::Acquire) && !HOLD_PENDING.swap(true, Ordering::AcqRel) {
            std::thread::spawn(|| {
                set_error(hold().err());
                HOLD_PENDING.store(false, Ordering::Release);
            });
        }
    } else if should_schedule_release(
        was,
        OWNED.load(Ordering::Acquire),
        RELEASE_TIMERS.load(Ordering::Acquire),
    ) {
        let epoch = RELEASE_EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
        RELEASE_TIMERS.fetch_add(1, Ordering::AcqRel);
        std::thread::spawn(move || {
            std::thread::sleep(GRACE);
            let result = release(Some(epoch));
            RELEASE_TIMERS.fetch_sub(1, Ordering::AcqRel);
            if let Err(error) = result {
                set_error(Some(error));
            }
        });
    }
}

fn release_is_due(timer_epoch: u64, current_epoch: u64, working: bool) -> bool {
    timer_epoch == current_epoch && !working
}

/// `SleepDisabled` from `pmset -g`: whether sleep is off right now.
fn sleep_disabled() -> Result<bool, String> {
    let output = Command::new(PMSET)
        .arg("-g")
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("Could not read the Mac's sleep setting.".into());
    }
    Ok(parse_sleep_disabled(&String::from_utf8_lossy(
        &output.stdout,
    )))
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

#[cfg(target_os = "macos")]
fn lid_closed() -> bool {
    macos::lid_closed().unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn lid_closed() -> bool {
    false
}

/// Never blank an open laptop or a docked external monitor. Keep checking
/// the actual display state: another app can wake it after the lid closes.
#[derive(Clone, Copy, Debug, Default)]
struct LidDisplayState {
    closed: bool,
    internal_awake: bool,
    external_connected: bool,
}

fn should_sleep_display(keep_awake_active: bool, state: LidDisplayState) -> bool {
    keep_awake_active && state.closed && state.internal_awake && !state.external_connected
}

fn should_sleep_system(state: LidDisplayState) -> bool {
    state.closed && !state.external_connected
}

fn display_guard_active() -> bool {
    ENABLED.load(Ordering::Acquire)
        && OWNED.load(Ordering::Acquire)
        && !SHUTTING_DOWN.load(Ordering::Acquire)
}

#[cfg(target_os = "macos")]
fn start_display_monitor() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(|| {
        let mut reported_error = false;
        // One thread for the process lifetime. Shutdown deactivates the
        // guard; a later init can reactivate it without spawning a duplicate.
        loop {
            // No subprocesses while idle or disabled; the native queries
            // below only run while keep-awake is active.
            if display_guard_active()
                && macos::lid_display_state().is_some_and(|state| should_sleep_display(true, state))
                && display_guard_active()
                && lid_closed()
            {
                let result = Command::new(PMSET)
                    .arg("displaysleepnow")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let failed = !result.is_ok_and(|status| status.success());
                if failed && !reported_error {
                    eprintln!("zeron: could not put the closed-lid display to sleep; will retry");
                }
                reported_error = failed;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn start_display_monitor() {}

#[cfg(target_os = "macos")]
mod macos {
    use super::LidDisplayState;
    use core_foundation::base::{CFAllocatorRef, CFType, CFTypeRef, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFMutableDictionaryRef;
    use core_foundation::string::{CFString, CFStringRef};
    use core_graphics::display::CGDisplay;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const std::ffi::c_char) -> CFMutableDictionaryRef;
        fn IOServiceGetMatchingService(port: u32, matching: CFMutableDictionaryRef) -> u32;
        fn IORegistryEntryCreateCFProperty(
            entry: u32,
            key: CFStringRef,
            allocator: CFAllocatorRef,
            options: u32,
        ) -> CFTypeRef;
        fn IOObjectRelease(object: u32) -> i32;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGGetOnlineDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    }

    pub(super) fn online_displays() -> Option<Vec<u32>> {
        // Unlike the active/drawable list, this includes hardware mirrors
        // and non-drawable built-in panels. If the list is truncated or the
        // query fails, do not risk blanking an unobserved external monitor.
        let mut displays = [0; 32];
        let mut count = 0;
        let result = unsafe {
            CGGetOnlineDisplayList(displays.len() as u32, displays.as_mut_ptr(), &mut count)
        };
        (result == 0 && (count as usize) < displays.len())
            .then(|| displays[..count as usize].to_vec())
    }

    pub(super) fn lid_closed() -> Option<bool> {
        // IOServiceGetMatchingService consumes the matching dictionary.
        // The property follows CF's Create rule; wrap it to release it.
        unsafe {
            let matching = IOServiceMatching(c"IOPMrootDomain".as_ptr());
            if matching.is_null() {
                return None;
            }
            let root = IOServiceGetMatchingService(0, matching);
            if root == 0 {
                return None;
            }
            let key = CFString::new("AppleClamshellState");
            let property = IORegistryEntryCreateCFProperty(
                root,
                key.as_concrete_TypeRef(),
                std::ptr::null(),
                0,
            );
            IOObjectRelease(root);
            if property.is_null() {
                return None;
            }
            CFType::wrap_under_create_rule(property)
                .downcast_into::<CFBoolean>()
                .map(bool::from)
        }
    }

    pub(super) fn lid_display_state() -> Option<LidDisplayState> {
        let closed = lid_closed()?;
        if !closed {
            return Some(LidDisplayState::default());
        }
        let mut state = LidDisplayState {
            closed,
            ..LidDisplayState::default()
        };
        for id in online_displays()? {
            let display = CGDisplay::new(id);
            if display.is_builtin() {
                state.internal_awake |= !display.is_asleep();
            } else {
                state.external_connected = true;
            }
        }
        Some(state)
    }
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
    ["1", "0"].into_iter().all(|value| {
        Command::new("/usr/bin/sudo")
            .args(["-n", "-l", PMSET, "disablesleep", value])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

/// The sudoers rule: this user may run exactly the two pmset commands.
fn sudoers_rule(uid: u32) -> String {
    format!("#{uid} ALL=(root) NOPASSWD: {PMSET} disablesleep 1, {PMSET} disablesleep 0")
}

/// Make sure the rule exists, installing it behind the macOS admin prompt.
/// The rule is validated with `visudo -c` before it goes live, so a bad file
/// can never break sudo.
fn prepare() -> Result<(), String> {
    if rule_allows() {
        return Ok(());
    }
    // Build and validate the rule entirely in a root-owned directory. No file
    // controlled by the requesting user is ever copied into sudoers.
    let uid = unsafe { libc::getuid() };
    let rule = sudoers_rule(uid);
    let script = format!(
        r#"set -eu; umask 077; f=$(/usr/bin/mktemp /etc/sudoers.d/.zeron-awake.XXXXXX); trap '/bin/rm -f "$f"' EXIT; /bin/echo '{rule}' > "$f"; /usr/sbin/visudo -cf "$f"; /bin/chmod 0440 "$f"; /bin/mv -f "$f" {SUDOERS_FILE}-{uid}"#
    );
    let apple = format!(
        r#"do shell script {} with prompt "Allow Zeron to prevent system sleep while local agents work?" with administrator privileges"#,
        serde_json::to_string(&script).unwrap()
    );
    let output = Command::new("/usr/bin/osascript")
        .args(["-e", &apple])
        .output()
        .map_err(|error| error.to_string())?;
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
    if OWNED.load(Ordering::Acquire)
        || !ENABLED.load(Ordering::Acquire)
        || !WORKING.load(Ordering::Acquire)
        || SHUTTING_DOWN.load(Ordering::Acquire)
    {
        return Ok(());
    }
    if sleep_disabled()? {
        // Someone else already holds it; never undo a choice that isn't ours.
        return Ok(());
    }
    // Arm the crash recovery before changing system sleep, not after it.
    spawn_watchdog()?;
    if sudo_pmset(&["disablesleep", "1"]).is_err() {
        let _ = std::fs::remove_file(marker_path());
        return Err("Could not disable sleep.".into());
    }
    OWNED.store(true, Ordering::Release);
    Ok(())
}

/// Give sleep back. A grace epoch means work ended: only that still-current
/// timer may release, and a closed undocked laptop should now sleep.
fn release(grace_epoch: Option<u64>) -> Result<(), String> {
    let _guard = PMSET_LOCK.lock().map_err(|_| "lock poisoned".to_string())?;
    // A timer may have fired just as the next turn started (and possibly
    // ended). Recheck its epoch under the same lock as hold().
    let still_due = || {
        grace_epoch.is_some_and(|epoch| {
            release_is_due(
                epoch,
                RELEASE_EPOCH.load(Ordering::Acquire),
                WORKING.load(Ordering::Acquire),
            )
        })
    };
    if grace_epoch.is_some() && !still_due() {
        return Ok(());
    }
    if !OWNED.swap(false, Ordering::AcqRel) {
        return Ok(());
    }
    if sudo_pmset(&["disablesleep", "0"]).is_err() {
        OWNED.store(true, Ordering::Release);
        return Err("Could not restore sleep.".into());
    }
    let _ = std::fs::remove_file(marker_path());
    #[cfg(target_os = "macos")]
    if still_due() && macos::lid_display_state().is_some_and(should_sleep_system) && still_due() {
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
fn spawn_watchdog() -> Result<(), String> {
    static SPAWNED: AtomicBool = AtomicBool::new(false);
    let marker = marker_path();
    std::fs::write(&marker, b"").map_err(|e| e.to_string())?;
    if SPAWNED.load(Ordering::Acquire) {
        return Ok(());
    }
    let pid = std::process::id();
    let marker = marker.to_string_lossy().into_owned();
    if marker.contains('\'') {
        return Err("unsupported temp path".into());
    }
    let script = format!(
        "while /bin/kill -0 {pid} 2>/dev/null; do /bin/sleep 5; done; \
         if [ -e '{marker}' ]; then /bin/rm -f '{marker}'; /usr/bin/sudo -n {PMSET} disablesleep 0; fi"
    );
    Command::new("/bin/sh")
        .args(["-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    SPAWNED.store(true, Ordering::Release);
    Ok(())
}

/// Present while Zeron holds sleep off; the watchdog acts only on it.
fn marker_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("zeron-keep-awake-held-{}", std::process::id()))
}

/// Restore sleep on a clean quit (the watchdog covers everything else).
pub fn shutdown() {
    SHUTTING_DOWN.store(true, Ordering::Release);
    ENABLED.store(false, Ordering::Release);
    RELEASE_EPOCH.fetch_add(1, Ordering::AcqRel);
    let _ = release(None);
    // If restoring sleep failed, leave the marker for the watchdog to retry.
    if !holding() {
        let _ = std::fs::remove_file(marker_path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_awake_tracks_fresh_local_work_including_background_tasks() {
        let now = Utc::now();
        for status in ["working", "awaitingInput", "idle", "errored"] {
            for children in [0, 1] {
                for background in [false, true] {
                    for waiting in [false, true] {
                        for age in [-30, 0, 45, 46, 600] {
                            for device in ["here", "remote"] {
                                let updated = now - chrono::Duration::seconds(age);
                                let mut row = serde_json::json!({ "chatId": "c", "deviceId": device, "status": status,
                                    "updatedAt": updated, "runningSubagents": children });
                                if background {
                                    row["backgroundTasks"] = serde_json::json!([{ "id": "b", "command": "cargo test", "startedAt": updated }]);
                                }
                                if waiting {
                                    row["waiting"] =
                                        serde_json::json!({ "since": updated, "on": [] });
                                }
                                let s: Session = serde_json::from_value(row).unwrap();
                                let work =
                                    status == "working" || children > 0 || background || waiting;
                                assert_eq!(
                                    activity(Some("here"), [&s], now).0,
                                    device == "here" && age <= 45 && work,
                                    "{status}, children={children}, background={background}, waiting={waiting}, age={age}, device={device}"
                                );
                                assert_eq!(activity(None, [&s], now), (false, None));
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn idle_background_work_expires_and_revives_without_polling() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let mut s: Session = serde_json::from_value(serde_json::json!({ "chatId": "c", "deviceId": "here", "status": "idle",
            "updatedAt": now, "backgroundTasks": [{ "id": "b", "command": "cargo test", "startedAt": now }] })).unwrap();
        let cutoff = now.timestamp() + 46;
        assert_eq!(activity(Some("here"), [&s], now), (true, Some(cutoff)));
        let at = DateTime::from_timestamp(cutoff, 0).unwrap();
        assert_eq!(activity(Some("here"), [&s], at), (false, None));
        s.updated_at = at;
        assert_eq!(activity(Some("here"), [&s], at), (true, Some(cutoff + 46)));
    }

    #[test]
    fn reads_sleep_disabled_from_pmset_output() {
        assert!(parse_sleep_disabled(
            " SleepDisabled\t\t1\n displaysleep 10\n"
        ));
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
    fn only_blanks_the_awake_internal_screen_with_a_closed_lid() {
        let closed = LidDisplayState {
            closed: true,
            internal_awake: true,
            external_connected: false,
        };
        assert!(should_sleep_display(true, closed));
        assert!(!should_sleep_display(false, closed));
        assert!(!should_sleep_display(
            true,
            LidDisplayState {
                closed: false,
                ..closed
            }
        ));
        assert!(!should_sleep_display(
            true,
            LidDisplayState {
                internal_awake: false,
                ..closed
            }
        ));
        assert!(!should_sleep_display(
            true,
            LidDisplayState {
                external_connected: true,
                ..closed
            }
        ));
    }

    #[test]
    fn reblanks_if_an_app_wakes_the_screen_under_the_closed_lid() {
        let mut state = LidDisplayState {
            closed: true,
            internal_awake: true,
            external_connected: false,
        };
        assert!(should_sleep_display(true, state));
        state.internal_awake = false;
        assert!(!should_sleep_display(true, state));
        state.internal_awake = true;
        assert!(should_sleep_display(true, state));
    }

    #[test]
    fn completed_work_does_not_force_a_docked_mac_to_sleep() {
        let mut state = LidDisplayState {
            closed: true,
            internal_awake: false,
            external_connected: false,
        };
        assert!(should_sleep_system(state));
        state.external_connected = true;
        assert!(!should_sleep_system(state));
        state.external_connected = false;
        state.closed = false;
        assert!(!should_sleep_system(state));
    }

    #[test]
    fn stale_grace_timers_never_release_a_newer_hold() {
        assert!(release_is_due(1, 1, false));
        assert!(!release_is_due(1, 1, true));
        // Idle -> working -> idle again: the first timer must not release
        // the second turn's grace period early.
        assert!(!release_is_due(1, 3, false));
        assert!(release_is_due(3, 3, false));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_online_query_includes_all_drawable_displays() {
        let online = macos::online_displays().expect("online displays");
        for active in core_graphics::display::CGDisplay::active_displays().unwrap() {
            assert!(online.contains(&active));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_lid_query_agrees_with_ioreg_without_changing_power_settings() {
        let out = Command::new("/usr/sbin/ioreg")
            .args(["-r", "-k", "AppleClamshellState", "-d", "1"])
            .output()
            .unwrap();
        assert!(out.status.success());
        let output = String::from_utf8_lossy(&out.stdout);
        let expected = output
            .contains("\"AppleClamshellState\"")
            .then(|| parse_lid_closed(&output));
        assert_eq!(macos::lid_closed(), expected);
    }

    #[test]
    fn sudoers_rule_names_only_the_two_pmset_commands() {
        assert_eq!(
            sudoers_rule(501),
            "#501 ALL=(root) NOPASSWD: /usr/bin/pmset disablesleep 1, /usr/bin/pmset disablesleep 0"
        );
    }

    #[test]
    fn idle_mac_never_schedules_spurious_release_and_retries_a_stuck_hold() {
        assert!(!should_schedule_release(false, false, 0));
        assert!(should_schedule_release(true, false, 0));
        assert!(should_schedule_release(true, true, 1));
        assert!(should_schedule_release(false, true, 0));
        assert!(!should_schedule_release(false, true, 1));
    }
}
