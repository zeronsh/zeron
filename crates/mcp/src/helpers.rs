//! Native helpers around computer use, bundled in
//! `Contents/Resources` next to the cua-driver:
//!
//! - `zeron-window-indicator` holds a ScreenCaptureKit stream on the window
//!   the agent drives, so macOS shows its capture indicator — the purple pill
//!   in that window's title bar — as it does for Synara's live preview. It
//!   follows the agent to whichever window it targets.
//! - `zeron-escape-monitor` is the kill switch: a passive event tap that
//!   reports the human's physical Escape while the agent works (Synara's
//!   EscapeKillSwitchMonitor). The bridge then interrupts the driver's input.
//!
//! Both stop after [`IDLE`] without computer calls and at the end of a turn.
//! `ZERON_WINDOW_INDICATOR=0` / `ZERON_ESCAPE_MONITOR=0` turn them off;
//! `ZERON_WINDOW_INDICATOR_BIN` / `ZERON_ESCAPE_MONITOR_BIN` point at
//! specific binaries.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Weak};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc};
use tokio::time::Instant;

/// How long a helper outlives the last computer call: matches the agent
/// cursor's idle hide, so they disappear together between steps.
pub const IDLE: Duration = Duration::from_secs(60);

const SWEEP: Duration = Duration::from_secs(5);

/// The window a call targets: Synara's frame-tap rule — both a valid `pid`
/// and `window_id` in the arguments.
pub fn target(arguments: &Value) -> Option<(u32, u32)> {
    let pid = arguments.get("pid")?.as_u64()?;
    let window = arguments.get("window_id")?.as_u64()?;
    let pid = u32::try_from(pid)
        .ok()
        .filter(|&p| p > 0 && p <= i32::MAX as u32)?;
    let window = u32::try_from(window).ok().filter(|&w| w > 0)?;
    Some((pid, window))
}

/// A bundled helper binary, unless switched off or missing.
fn locate(name: &str, off_var: &str, bin_var: &str) -> Option<PathBuf> {
    let off =
        std::env::var(off_var).is_ok_and(|v| matches!(v.trim(), "0" | "false" | "off" | "no"));
    if off {
        return None;
    }
    let binary = match std::env::var_os(bin_var) {
        Some(path) => PathBuf::from(path),
        None => {
            let exe = std::env::current_exe().ok()?;
            let exe = exe.canonicalize().unwrap_or(exe);
            let dir = exe.parent()?;
            // Zeron.app/Contents/MacOS/zeron → Contents/Resources/
            [dir.join("../Resources").join(name), dir.join(name)]
                .into_iter()
                .find(|path| path.is_file())?
        }
    };
    binary.is_file().then_some(binary)
}

/// One running helper. Dropping it closes the helper's stdin (it exits on
/// its own); kill_on_drop backs that up for a wedged helper. Helpers own no
/// children, so a kill orphans nothing.
struct Running<K> {
    key: K,
    last_used: Instant,
    child: Child,
}

impl<K> Running<K> {
    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

/// Periodically drop a helper idle for longer than `idle`.
fn spawn_sweep<T: Send + Sync + 'static, K: Send + 'static>(
    owner: Weak<T>,
    slot: fn(&T) -> &Mutex<Option<Running<K>>>,
    idle: Duration,
) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(SWEEP.min(idle)).await;
            let Some(owner) = owner.upgrade() else {
                return;
            };
            let mut running = slot(&owner).lock().await;
            if running
                .as_ref()
                .is_some_and(|r| r.last_used.elapsed() >= idle)
            {
                *running = None;
            }
        }
    });
}

/// The capture indicator on the window being driven.
pub struct Indicator {
    binary: PathBuf,
    active: Mutex<Option<Running<(u32, u32)>>>,
}

impl Indicator {
    pub fn locate() -> Option<Arc<Self>> {
        locate(
            "zeron-window-indicator",
            "ZERON_WINDOW_INDICATOR",
            "ZERON_WINDOW_INDICATOR_BIN",
        )
        .map(|binary| Self::new(binary, IDLE))
    }

    pub fn new(binary: PathBuf, idle: Duration) -> Arc<Self> {
        let indicator = Arc::new(Self {
            binary,
            active: Mutex::new(None),
        });
        spawn_sweep(Arc::downgrade(&indicator), |i: &Self| &i.active, idle);
        indicator
    }

    /// A computer call is about to act on `target`: show the indicator there
    /// (moving it from any previous window) and keep it alive.
    pub async fn touch(&self, target: (u32, u32)) {
        let mut active = self.active.lock().await;
        if let Some(current) = active.as_mut()
            && current.key == target
            && current.alive()
        {
            current.last_used = Instant::now();
            return;
        }
        *active = None;
        let (pid, window) = target;
        let mut command = Command::new(&self.binary);
        command
            .arg("--window")
            .arg(window.to_string())
            .arg("--pid")
            .arg(pid.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        match command.spawn() {
            Ok(child) => {
                *active = Some(Running {
                    key: target,
                    last_used: Instant::now(),
                    child,
                })
            }
            Err(error) => tracing::debug!(%error, "computer use: window indicator failed to start"),
        }
    }

    /// The turn ended: nothing is being driven any more.
    pub async fn stop(&self) {
        *self.active.lock().await = None;
    }

    /// The target currently shown, if its helper is still running.
    #[cfg(test)]
    async fn current(&self) -> Option<(u32, u32)> {
        let mut active = self.active.lock().await;
        if !active.as_mut().is_some_and(Running::alive) {
            *active = None;
        }
        active.as_ref().map(|a| a.key)
    }
}

/// The physical-Escape kill switch, armed while the agent works.
pub struct EscapeMonitor {
    binary: PathBuf,
    armed: Mutex<Option<Running<()>>>,
    presses: mpsc::UnboundedSender<()>,
}

impl EscapeMonitor {
    pub fn locate(presses: mpsc::UnboundedSender<()>) -> Option<Arc<Self>> {
        locate(
            "zeron-escape-monitor",
            "ZERON_ESCAPE_MONITOR",
            "ZERON_ESCAPE_MONITOR_BIN",
        )
        .map(|binary| Self::new(binary, IDLE, presses))
    }

    pub fn new(binary: PathBuf, idle: Duration, presses: mpsc::UnboundedSender<()>) -> Arc<Self> {
        let monitor = Arc::new(Self {
            binary,
            armed: Mutex::new(None),
            presses,
        });
        spawn_sweep(Arc::downgrade(&monitor), |m: &Self| &m.armed, idle);
        monitor
    }

    /// Computer work is happening: listen for Escape (or keep listening).
    pub async fn arm(&self) {
        let mut armed = self.armed.lock().await;
        if let Some(running) = armed.as_mut()
            && running.alive()
        {
            running.last_used = Instant::now();
            return;
        }
        let mut command = Command::new(&self.binary);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                tracing::debug!(%error, "computer use: escape monitor failed to start");
                *armed = None;
                return;
            }
        };
        if let Some(stdout) = child.stdout.take() {
            let presses = self.presses.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    match line.trim() {
                        "escape" => {
                            if presses.send(()).is_err() {
                                return;
                            }
                        }
                        "ready" => {}
                        other => tracing::debug!(status = other, "computer use: escape monitor"),
                    }
                }
            });
        }
        *armed = Some(Running {
            key: (),
            last_used: Instant::now(),
            child,
        });
    }

    /// The turn ended: stop listening to the keyboard.
    pub async fn disarm(&self) {
        *self.armed.lock().await = None;
    }

    #[cfg(test)]
    async fn is_armed(&self) -> bool {
        let mut armed = self.armed.lock().await;
        if !armed.as_mut().is_some_and(Running::alive) {
            *armed = None;
        }
        armed.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn targets_need_a_pid_and_a_window() {
        assert_eq!(
            target(&json!({ "pid": 8071, "window_id": 47616 })),
            Some((8071, 47616))
        );
        assert_eq!(target(&json!({ "pid": 8071 })), None);
        assert_eq!(target(&json!({ "window_id": 1, "pid": 0 })), None);
        assert_eq!(target(&json!({ "window_id": "1", "pid": 2 })), None);
        assert_eq!(
            target(&json!({ "window_id": 1, "pid": u64::from(u32::MAX) })),
            None
        );
    }

    /// Stand-in helper script: records its arguments, then runs `body`.
    fn fake_helper(dir: &tempfile::TempDir, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.path().join(name);
        let log = dir.path().join("spawns.log");
        std::fs::write(
            &path,
            format!("#!/bin/sh\necho \"$@\" >> '{}'\n{body}\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    async fn wait_for_spawns(dir: &tempfile::TempDir, count: usize) -> Vec<String> {
        let read = || {
            std::fs::read_to_string(dir.path().join("spawns.log"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        for _ in 0..500 {
            if read().len() >= count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        read()
    }

    #[tokio::test]
    async fn indicator_follows_the_target_and_expires_when_idle() {
        let dir = tempfile::tempdir().unwrap();
        let helper = fake_helper(&dir, "zeron-window-indicator", "cat >/dev/null");
        // Long enough that a shell slow to start under a loaded parallel test
        // run is not swept before it logs its spawn.
        let indicator = Indicator::new(helper, Duration::from_secs(2));
        indicator.touch((10, 20)).await;
        assert_eq!(wait_for_spawns(&dir, 1).await, ["--window 20 --pid 10"]);
        indicator.touch((10, 20)).await;
        assert_eq!(indicator.current().await, Some((10, 20)));
        indicator.touch((10, 21)).await;
        assert_eq!(
            wait_for_spawns(&dir, 2).await,
            ["--window 20 --pid 10", "--window 21 --pid 10"],
            "one helper per target, reused while it stays the same"
        );
        assert_eq!(indicator.current().await, Some((10, 21)));
        // Expiry is noticed within one idle period plus one sweep (both 2s).
        tokio::time::sleep(Duration::from_millis(4500)).await;
        assert_eq!(indicator.current().await, None, "idle indicator stopped");
        indicator.touch((10, 21)).await;
        indicator.stop().await;
        assert_eq!(indicator.current().await, None, "stopped at turn end");
    }

    #[tokio::test]
    async fn escape_monitor_reports_presses_until_disarmed() {
        let dir = tempfile::tempdir().unwrap();
        // Reports one press, then lives until its stdin closes.
        let helper = fake_helper(
            &dir,
            "zeron-escape-monitor",
            "echo ready\necho escape\ncat >/dev/null",
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        let monitor = EscapeMonitor::new(helper, IDLE, tx);
        monitor.arm().await;
        monitor.arm().await;
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("escape forwarded")
            .unwrap();
        assert_eq!(
            wait_for_spawns(&dir, 1).await.len(),
            1,
            "re-arming reuses the listener"
        );
        assert!(monitor.is_armed().await);
        monitor.disarm().await;
        assert!(!monitor.is_armed().await);
    }
}
