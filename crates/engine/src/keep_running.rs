//! "Keep Zeron running after you quit" (Settings → General), this device's
//! own setting for its background engine ([`crate::managed`]).
//!
//! On (the default): quitting the app leaves the engine running, as it
//! always has — chats keep working, other devices keep reaching this one —
//! and idle agents are stopped as they would be anyway
//! ([`crate::sessions::AGENT_IDLE_STOP`]).
//!
//! Off: the app tells the engine as it quits ([`APP_QUITTING`]), and the
//! engine stops once nothing is pending. Work still running when the app
//! quits is never ended silently: the app asks first ("keep running until
//! they finish, or stop them now?"), and only "stop them now" stops it —
//! the person's own Stop on each chat. Any later call from this device's
//! app (it opened again) calls the stop off.
//!
//! [`APP_QUITTING`]: zeron_rpc::methods::APP_QUITTING

use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::{Deserialize, Serialize};

const FILE: &str = "background.json";

/// How long the engine must find nothing pending before it stops: a queued
/// message dispatches, a wake turn starts, within this.
pub const QUIET_BEFORE_STOP: Duration = Duration::from_secs(5);
/// How often a quit-when-idle engine looks.
const CHECK_EVERY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundSettings {
    #[serde(default = "on")]
    pub keep_running_after_quit: bool,
}

fn on() -> bool {
    true
}

impl Default for BackgroundSettings {
    fn default() -> Self {
        Self {
            keep_running_after_quit: true,
        }
    }
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// The saved setting; the default when none is saved (or it can't be read).
pub fn load(data_dir: &Path) -> BackgroundSettings {
    std::fs::read(path(data_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Save it (atomically: a sibling file renamed over).
pub fn save(data_dir: &Path, settings: &BackgroundSettings) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let tmp = data_dir.join(format!(".{FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(settings)?)?;
    std::fs::rename(&tmp, path(data_dir))
}

/// What a quitting app is told, and decides on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuitCheck {
    pub keep_running: bool,
    /// Chats whose work is still running or pending.
    pub working: usize,
}

/// What the engine has pending: turns running or waiting on a question,
/// and runtimes whose turn ended with background work still live.
pub fn pending(sessions: &crate::sessions::SessionsEngine) -> usize {
    sessions.chats_with_work().len()
}

/// A quit-when-idle request, shared between the RPC that sets it and the
/// watcher that acts on it.
#[derive(Clone)]
pub struct QuitWhenIdle(Arc<tokio::sync::watch::Sender<bool>>);

impl Default for QuitWhenIdle {
    fn default() -> Self {
        Self(Arc::new(tokio::sync::watch::Sender::new(false)))
    }
}

impl QuitWhenIdle {
    pub fn arm(&self) {
        self.0.send_replace(true);
    }

    /// The app is back: keep running.
    pub fn disarm(&self) -> bool {
        self.0.send_replace(false)
    }

    pub fn armed(&self) -> bool {
        *self.0.borrow()
    }
}

/// Until armed and nothing has been pending for [`QUIET_BEFORE_STOP`]:
/// then the engine stops. `pending` is read every [`CHECK_EVERY`] while
/// armed; unarmed (the app open, or set to keep running) it sleeps until
/// armed, rather than waking the engine every second to find it isn't.
pub async fn wait_until_idle_after_quit(quit: QuitWhenIdle, pending: impl Fn() -> usize) {
    let mut quiet_since: Option<tokio::time::Instant> = None;
    let mut armed = quit.0.subscribe();
    let mut tick = tokio::time::interval(CHECK_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        if !*armed.borrow_and_update() {
            quiet_since = None;
            // (The sender lives in `quit`, held here: never closed.)
            let _ = armed.changed().await;
            tick.reset();
            continue;
        }
        tick.tick().await;
        if !quit.armed() || pending() > 0 {
            quiet_since = None;
            continue;
        }
        let since = *quiet_since.get_or_insert_with(tokio::time::Instant::now);
        if since.elapsed() >= QUIET_BEFORE_STOP {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn the_setting_is_on_until_turned_off_and_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).keep_running_after_quit);
        save(dir.path(), &BackgroundSettings { keep_running_after_quit: false }).unwrap();
        assert!(!load(dir.path()).keep_running_after_quit);
        std::fs::write(path(dir.path()), b"not json").unwrap();
        assert!(load(dir.path()).keep_running_after_quit, "unreadable reads as the default");
    }

    #[tokio::test(start_paused = true)]
    async fn a_quit_engine_stops_only_once_its_work_is_done() {
        let quit = QuitWhenIdle::default();
        let work = Arc::new(AtomicUsize::new(2));
        let waiting = {
            let (quit, work) = (quit.clone(), work.clone());
            tokio::spawn(wait_until_idle_after_quit(quit, move || work.load(Ordering::SeqCst)))
        };
        // Not armed: it never stops, idle or not.
        work.store(0, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(!waiting.is_finished());
        // Armed with work running: it waits for the work.
        work.store(2, Ordering::SeqCst);
        quit.arm();
        tokio::time::sleep(Duration::from_secs(3600)).await;
        assert!(!waiting.is_finished(), "stopped with work running");
        // The work ends: stopped after a quiet stretch, not at once.
        work.store(0, Ordering::SeqCst);
        tokio::time::sleep(QUIET_BEFORE_STOP - Duration::from_secs(2)).await;
        assert!(!waiting.is_finished());
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert!(waiting.is_finished());
    }

    #[tokio::test(start_paused = true)]
    async fn the_app_coming_back_calls_the_stop_off() {
        let quit = QuitWhenIdle::default();
        let waiting = tokio::spawn(wait_until_idle_after_quit(quit.clone(), || 0));
        quit.arm();
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(quit.disarm());
        tokio::time::sleep(Duration::from_secs(600)).await;
        assert!(!waiting.is_finished());
    }
}
