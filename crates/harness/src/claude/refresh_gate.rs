//! Claude launches take turns through an expired login.
//!
//! Claude Code refreshes its OAuth login lazily, from whichever process needs
//! it first, and a refresh token is single-use: two concurrent refreshes with
//! the same one revoke the login (why the engine's `refresh_claude_slot` runs
//! one refresh per slot at a time). Zeron launches concurrently as a matter of
//! course (a new chat starts its run and its title run in the same second),
//! and the revocation is silent: the winning access token
//! works for eight hours, then every run fails with "OAuth session expired
//! and could not be refreshed" until the user signs in again.
//!
//! So while the stored login is due a refresh (expired, or inside the CLI's
//! own five-minute refresh-ahead window), launches take turns. The first
//! starts at once and refreshes; the rest start as soon as the credentials
//! file holds a current token again, or the first run hands its turn back by
//! reaching the API or ending. A current login costs one small file read.
//!
//! Only the credentials file is consulted. macOS keeps the live login in the
//! Keychain, where the file is at most a stale fallback, so launches there
//! pass straight through.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::OwnedMutexGuard;

use crate::process::{Child, Command};

/// The CLI renews a login this close to its expiry.
const REFRESH_AHEAD: Duration = Duration::from_secs(5 * 60);
/// The longest a launch waits for the one ahead of it. A refresh takes a
/// second or two; a turn held longer is a run slow to reach the API, and
/// stalling every other launch behind it would cost more than it protects.
const TURN_WAIT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);

/// Spawn a Claude CLI process. When the stored login is due a refresh, the
/// launch comes back holding the [`Turn`]; keep it until the run reaches the
/// API ([`Turn::authenticated`]) or ends.
pub(super) async fn spawn(cmd: &mut Command) -> io::Result<(Child, Option<Turn>)> {
    if cfg!(target_os = "macos") {
        return cmd.spawn().map(|child| (child, None));
    }
    let config = crate::model_context::root(
        "CLAUDE_CONFIG_DIR",
        crate::executable::home_or_current_dir().join(".claude"),
    );
    launch(&config.join(".credentials.json"), || cmd.spawn()).await
}

/// A launch's claim on refreshing the stored login. Dropping it hands the
/// turn to the next launch.
pub(super) struct Turn {
    gate: Arc<Gate>,
    /// The stored login when this launch took the turn.
    taken: StoredLogin,
    _held: OwnedMutexGuard<()>,
}

impl Turn {
    /// The run reached the API. Doing so past the stored token's expiry
    /// without rewriting it means the CLI authenticates another way (an API
    /// key or token variable, say), so this file isn't the live login: stop
    /// gating on it until it changes. (Inside the refresh-ahead window the
    /// stored token may still have been good.)
    pub(super) fn authenticated(self) {
        let unrefreshed = modified(&self.gate.credentials) == Some(self.taken.modified);
        if unrefreshed && self.taken.expires_at <= SystemTime::now() {
            *lock(&self.gate.ignored) = Some(self.taken.modified);
        }
    }
}

#[derive(Clone, Copy)]
struct StoredLogin {
    modified: SystemTime,
    /// When its access token expires.
    expires_at: SystemTime,
}

struct Gate {
    credentials: PathBuf,
    turn: Arc<tokio::sync::Mutex<()>>,
    /// The mtime of an expired login that runs authenticated without refreshing.
    ignored: Mutex<Option<SystemTime>>,
}

impl Gate {
    /// The stored login, while it's due a refresh.
    fn due(&self) -> Option<StoredLogin> {
        let login = stored_login(&self.credentials)?;
        let due = login.expires_at <= SystemTime::now() + REFRESH_AHEAD;
        (due && *lock(&self.ignored) != Some(login.modified)).then_some(login)
    }
}

async fn launch<T>(
    credentials: &Path,
    spawn: impl FnOnce() -> io::Result<T>,
) -> io::Result<(T, Option<Turn>)> {
    let gate = gate(credentials);
    if gate.due().is_none() {
        return spawn().map(|launched| (launched, None));
    }
    let deadline = tokio::time::Instant::now() + TURN_WAIT;
    let held = loop {
        if let Ok(held) = gate.turn.clone().try_lock_owned() {
            break Some(held);
        }
        // The launch ahead refreshed it, or is taking too long to.
        if gate.due().is_none() || tokio::time::Instant::now() >= deadline {
            break None;
        }
        tokio::time::sleep(POLL).await;
    };
    let turn = held.and_then(|held| {
        let taken = gate.due()?;
        Some(Turn {
            gate: gate.clone(),
            taken,
            _held: held,
        })
    });
    spawn().map(|launched| (launched, turn))
}

/// One gate per credentials file: separate `CLAUDE_CONFIG_DIR` logins never
/// wait on each other.
fn gate(credentials: &Path) -> Arc<Gate> {
    static GATES: OnceLock<Mutex<HashMap<PathBuf, Arc<Gate>>>> = OnceLock::new();
    lock(GATES.get_or_init(Default::default))
        .entry(credentials.to_path_buf())
        .or_insert_with(|| {
            Arc::new(Gate {
                credentials: credentials.to_path_buf(),
                turn: Arc::default(),
                ignored: Mutex::new(None),
            })
        })
        .clone()
}

/// The stored OAuth login, if there is one.
fn stored_login(credentials: &Path) -> Option<StoredLogin> {
    let modified = modified(credentials)?;
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(credentials).ok()?).ok()?;
    let expires_ms = stored.get("claudeAiOauth")?.get("expiresAt")?.as_u64()?;
    Some(StoredLogin {
        modified,
        expires_at: UNIX_EPOCH + Duration::from_millis(expires_ms),
    })
}

fn modified(credentials: &Path) -> Option<SystemTime> {
    std::fs::metadata(credentials)
        .and_then(|meta| meta.modified())
        .ok()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stored login whose access token expires `expires_in` seconds from now.
    fn store_login(path: &Path, expires_in: i64) {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let login = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "access",
                "refreshToken": "refresh",
                "expiresAt": now_ms + expires_in * 1000,
            }
        });
        std::fs::write(path, login.to_string()).unwrap();
    }

    /// Give the file a distinct mtime, as any rewrite by the CLI would.
    fn touch(path: &Path, ahead: u64) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(ahead))
            .unwrap();
    }

    fn credentials() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".credentials.json");
        (dir, path)
    }

    async fn turn(path: &Path) -> Option<Turn> {
        launch(path, || Ok(())).await.unwrap().1
    }

    #[tokio::test]
    async fn a_current_or_absent_login_launches_straight_through() {
        let (_dir, path) = credentials();
        assert!(turn(&path).await.is_none(), "no stored login");
        std::fs::write(&path, "not json").unwrap();
        assert!(turn(&path).await.is_none(), "unreadable login");
        store_login(&path, 3600);
        assert!(turn(&path).await.is_none(), "current login");
        assert!(turn(&path).await.is_none(), "and nothing was held");
    }

    #[tokio::test]
    async fn launches_through_a_due_login_take_turns() {
        let (_dir, path) = credentials();
        // Inside the CLI's refresh-ahead window counts as due.
        store_login(&path, 60);
        let first = turn(&path).await.expect("the first launch refreshes");
        let waiting = tokio::spawn({
            let path = path.clone();
            async move { turn(&path).await }
        });
        tokio::time::sleep(POLL * 3).await;
        assert!(!waiting.is_finished(), "the next launch waits its turn");
        // The first run ended without refreshing: the next takes its place.
        drop(first);
        assert!(waiting.await.unwrap().is_some());
    }

    #[tokio::test]
    async fn a_refresh_releases_the_waiting_launches() {
        let (_dir, path) = credentials();
        store_login(&path, -60);
        let _first = turn(&path).await.expect("the first launch refreshes");
        let waiting = tokio::spawn({
            let path = path.clone();
            async move { turn(&path).await }
        });
        tokio::time::sleep(POLL * 3).await;
        assert!(!waiting.is_finished());
        // The first run's CLI stores its new token; its turn is still held.
        store_login(&path, 8 * 3600);
        assert!(
            waiting.await.unwrap().is_none(),
            "the login is current: nothing left to take turns over"
        );
    }

    #[tokio::test]
    async fn authenticating_without_a_refresh_stops_gating_that_file() {
        let (_dir, path) = credentials();
        store_login(&path, -60);
        turn(&path).await.expect("due").authenticated();
        // Runs authenticate some other way (an API key, say): stop gating.
        assert!(turn(&path).await.is_none());
        // A different login in the file is gated again.
        store_login(&path, -60);
        touch(&path, 5);
        assert!(turn(&path).await.is_some());
    }

    #[tokio::test]
    async fn a_run_on_a_still_valid_token_keeps_the_gate() {
        let (_dir, path) = credentials();
        // Due only by the refresh-ahead window: its token may still serve.
        store_login(&path, 60);
        turn(&path).await.expect("due").authenticated();
        assert!(turn(&path).await.is_some(), "still gated for the expiry");
    }

    #[tokio::test(start_paused = true)]
    async fn a_launch_waits_no_longer_than_the_cap() {
        let (_dir, path) = credentials();
        store_login(&path, -60);
        let _first = turn(&path).await.expect("due");
        let started = tokio::time::Instant::now();
        assert!(turn(&path).await.is_none(), "launches without a turn");
        assert!(started.elapsed() >= TURN_WAIT);
    }

    #[tokio::test]
    async fn separate_logins_never_wait_on_each_other() {
        let (_a, first) = credentials();
        let (_b, second) = credentials();
        store_login(&first, -60);
        store_login(&second, -60);
        let _held = turn(&first).await.expect("due");
        assert!(turn(&second).await.is_some(), "its own turn, at once");
    }
}
