//! Claude Code's OAuth refresh lock — honoured by everything zeron does to a
//! Claude login or a Claude process.
//!
//! A Claude.ai refresh token is single-use: every refresh rotates the pair,
//! and the CLI saves the new pair with a compare-and-swap that keeps the
//! store only while it still holds the refresh token that was posted
//! (2.1.x). The CLI answers an `invalid_grant` by wiping the stored login
//! ("Not logged in · Please run /login"). So a rotated pair that never
//! reaches the store logs the user out on the next refresh, and zeron used to
//! lose one in two ways:
//!
//! - **Killing a child mid-refresh.** SIGKILL saves nothing; on SIGTERM the
//!   CLI waits only 2s for an in-flight refresh before exiting without it.
//!   Short-lived probe children (`initialize`) start right when an expired
//!   token gets refreshed, and are torn down as soon as they answer.
//! - **Swapping the stored login mid-refresh.** The CLI's CAS then drops the
//!   rotated pair, so the saved slot of the old account keeps a spent refresh
//!   token — and switching back to it installs a dead login.
//!
//! The CLI serialises refreshes across processes with a proper-lockfile
//! directory lock: `<config>/.oauth_refresh.lock` (2.1.x; stale after 60s,
//! its mtime bumped every 5s while held), taken together with the legacy
//! `<realpath(config)>.lock` (older CLIs take only that one). [`settle`] waits
//! out a live holder before a Claude child is signalled; [`RefreshLock`] takes
//! both locks the way the CLI does, so a login swap never interleaves with a
//! refresh.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The current lock, inside the config dir.
const LOCK_NAME: &str = ".oauth_refresh.lock";
/// A holder bumps its lock's mtime every 5s; one untouched for longer than
/// this lost its holder (killed mid-refresh) and holds nothing up.
const LIVE_WINDOW: Duration = Duration::from_secs(20);
/// How often a held [`RefreshLock`] bumps its mtime (the CLI's is 5s).
const TOUCH_EVERY: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(150);
/// Longest [`settle`] wait before a Claude child is signalled anyway: the
/// CLI's refresh POST times out at 30s, then the profile fetch and the save.
pub const SETTLE_MAX: Duration = Duration::from_secs(40);

/// Claude Code's credential config dir: `CLAUDE_SECURESTORAGE_CONFIG_DIR`
/// (an empty value meaning `~/.claude`), else `CLAUDE_CONFIG_DIR`, else
/// `~/.claude` — the dir the CLI locks.
pub fn config_dir() -> PathBuf {
    config_dir_with(
        std::env::var_os("CLAUDE_SECURESTORAGE_CONFIG_DIR"),
        std::env::var_os("CLAUDE_CONFIG_DIR"),
    )
}

/// [`config_dir`] for a run whose own environment may relocate the config.
pub(crate) fn config_dir_for(env: &std::collections::BTreeMap<String, String>) -> PathBuf {
    let var = |name: &str| {
        env.get(name)
            .map(OsString::from)
            .or_else(|| std::env::var_os(name))
    };
    config_dir_with(
        var("CLAUDE_SECURESTORAGE_CONFIG_DIR"),
        var("CLAUDE_CONFIG_DIR"),
    )
}

fn config_dir_with(secure_storage: Option<OsString>, config: Option<OsString>) -> PathBuf {
    let default = || crate::executable::home_or_current_dir().join(".claude");
    match secure_storage {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        Some(_) => default(),
        None => config
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(default),
    }
}

/// Both lock paths for `config_dir`: the current one, then the legacy one.
pub fn lock_paths(config_dir: &Path) -> [PathBuf; 2] {
    let real = std::fs::canonicalize(config_dir).unwrap_or_else(|_| config_dir.to_path_buf());
    let mut legacy = real.into_os_string();
    legacy.push(".lock");
    [config_dir.join(LOCK_NAME), PathBuf::from(legacy)]
}

/// Lock age, `None` when there is no lock at `path`.
fn lock_age(path: &Path) -> Option<Duration> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(
        SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default(),
    )
}

/// Whether a live Claude process is refreshing the login in `config_dir`.
pub fn refresh_in_flight(config_dir: &Path) -> bool {
    lock_paths(config_dir)
        .iter()
        .any(|path| lock_age(path).is_some_and(|age| age < LIVE_WINDOW))
}

/// Wait (at most `max`) until no live process holds the refresh lock.
/// `true` = idle; `false` = still held when `max` ran out.
pub async fn settle(config_dir: &Path, max: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + max;
    let mut waited = false;
    while refresh_in_flight(config_dir) {
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(
                target: "zeron_harness::claude",
                dir = %config_dir.display(),
                "Claude login refresh still in flight; proceeding"
            );
            return false;
        }
        if !waited {
            waited = true;
            tracing::info!(
                target: "zeron_harness::claude",
                dir = %config_dir.display(),
                "waiting for an in-flight Claude login refresh to be saved"
            );
        }
        tokio::time::sleep(POLL).await;
    }
    true
}

/// Both of Claude Code's refresh locks, held. While held no CLI refreshes
/// (each retries until it can take the lock, then re-reads the store), so
/// the stored login can be read and replaced as one step. Released on drop.
pub struct RefreshLock {
    held: Vec<PathBuf>,
    touch: Option<tokio::task::JoinHandle<()>>,
}

impl RefreshLock {
    /// Take both locks, waiting (at most `timeout`) for a live holder to
    /// finish. A lock whose holder died is taken over, as the CLI does.
    pub async fn acquire(config_dir: &Path, timeout: Duration) -> std::io::Result<Self> {
        std::fs::create_dir_all(config_dir)?;
        let paths = lock_paths(config_dir);
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let mut lock = Self {
                held: Vec::new(),
                touch: None,
            };
            let mut busy = false;
            for path in &paths {
                match try_take(path)? {
                    true => lock.held.push(path.clone()),
                    false => {
                        busy = true;
                        break;
                    }
                }
            }
            if !busy {
                let held = lock.held.clone();
                lock.touch = Some(tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(TOUCH_EVERY).await;
                        let now = filetime::FileTime::now();
                        for path in &held {
                            let _ = filetime::set_file_mtime(path, now);
                        }
                    }
                }));
                return Ok(lock);
            }
            // Release a partial hold before waiting: never sit on one lock
            // while the CLI holds the other.
            drop(lock);
            if tokio::time::Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "Claude Code is refreshing its login",
                ));
            }
            tokio::time::sleep(POLL).await;
        }
    }
}

/// `mkdir` the lock (proper-lockfile's primitive). `false` = a live holder
/// has it; a dead holder's lock is removed and taken.
fn try_take(path: &Path) -> std::io::Result<bool> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            match lock_age(path) {
                Some(age) if age < LIVE_WINDOW => Ok(false),
                // Gone in between, or abandoned: take it.
                _ => {
                    let _ = std::fs::remove_dir_all(path);
                    match std::fs::create_dir(path) {
                        Ok(()) => Ok(true),
                        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
                        Err(err) => Err(err),
                    }
                }
            }
        }
        Err(err) => Err(err),
    }
}

impl Drop for RefreshLock {
    fn drop(&mut self) {
        if let Some(touch) = self.touch.take() {
            touch.abort();
        }
        for path in self.held.drain(..).rev() {
            let _ = std::fs::remove_dir(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn age_lock(path: &Path, by: Duration) {
        let then = filetime::FileTime::from_system_time(SystemTime::now() - by);
        filetime::set_file_mtime(path, then).unwrap();
    }

    #[test]
    fn config_dir_follows_the_clis_precedence() {
        let home = crate::executable::home_or_current_dir().join(".claude");
        assert_eq!(config_dir_with(None, None), home);
        assert_eq!(config_dir_with(None, Some("".into())), home);
        assert_eq!(
            config_dir_with(None, Some("/c".into())),
            PathBuf::from("/c")
        );
        assert_eq!(
            config_dir_with(Some("/s".into()), Some("/c".into())),
            PathBuf::from("/s")
        );
        assert_eq!(config_dir_with(Some("".into()), Some("/c".into())), home);
    }

    #[test]
    fn lock_paths_name_the_current_and_legacy_locks() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("claude");
        std::fs::create_dir_all(&config).unwrap();
        let [current, legacy] = lock_paths(&config);
        assert_eq!(current, config.join(".oauth_refresh.lock"));
        let real = std::fs::canonicalize(&config).unwrap();
        assert_eq!(legacy, PathBuf::from(format!("{}.lock", real.display())));
    }

    #[tokio::test]
    async fn a_live_holder_is_waited_out_and_a_dead_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("claude");
        std::fs::create_dir_all(&config).unwrap();
        assert!(!refresh_in_flight(&config));

        let [current, legacy] = lock_paths(&config);
        std::fs::create_dir(&current).unwrap();
        assert!(refresh_in_flight(&config));
        assert!(!settle(&config, Duration::from_millis(300)).await);

        // The holder was killed: its lock stops being touched.
        age_lock(&current, Duration::from_secs(30));
        assert!(!refresh_in_flight(&config));
        assert!(settle(&config, Duration::from_millis(300)).await);

        // Older CLIs lock only the legacy path.
        std::fs::create_dir(&legacy).unwrap();
        assert!(refresh_in_flight(&config));
    }

    #[tokio::test]
    async fn settle_returns_once_the_holder_releases() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("claude");
        std::fs::create_dir_all(&config).unwrap();
        let [current, _] = lock_paths(&config);
        std::fs::create_dir(&current).unwrap();
        let release = current.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            std::fs::remove_dir(release).unwrap();
        });
        assert!(settle(&config, Duration::from_secs(5)).await);
    }

    #[tokio::test]
    async fn acquire_takes_both_locks_and_releases_them_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("claude");
        let lock = RefreshLock::acquire(&config, Duration::from_secs(1))
            .await
            .unwrap();
        let [current, legacy] = lock_paths(&config);
        assert!(current.is_dir() && legacy.is_dir());
        assert!(refresh_in_flight(&config));
        // A second taker (another CLI) waits.
        assert!(
            RefreshLock::acquire(&config, Duration::from_millis(300))
                .await
                .is_err()
        );
        drop(lock);
        assert!(!current.exists() && !legacy.exists());
    }

    #[tokio::test]
    async fn acquire_waits_for_a_live_holder_and_takes_over_a_dead_one() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("claude");
        std::fs::create_dir_all(&config).unwrap();
        let [current, legacy] = lock_paths(&config);

        // Live holder of the legacy lock only: nothing is left half-held.
        std::fs::create_dir(&legacy).unwrap();
        assert!(
            RefreshLock::acquire(&config, Duration::from_millis(300))
                .await
                .is_err()
        );
        assert!(!current.exists());

        // The holder died long ago.
        age_lock(&legacy, Duration::from_secs(120));
        let lock = RefreshLock::acquire(&config, Duration::from_secs(1))
            .await
            .unwrap();
        assert!(current.is_dir() && legacy.is_dir());
        drop(lock);
        assert!(!legacy.exists());
    }

    #[tokio::test]
    async fn a_held_lock_stays_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("claude");
        let lock = RefreshLock::acquire(&config, Duration::from_secs(1))
            .await
            .unwrap();
        let [current, legacy] = lock_paths(&config);
        age_lock(&current, Duration::from_secs(15));
        age_lock(&legacy, Duration::from_secs(15));
        tokio::time::sleep(TOUCH_EVERY + Duration::from_millis(300)).await;
        assert!(lock_age(&current).unwrap() < Duration::from_secs(5));
        assert!(lock_age(&legacy).unwrap() < Duration::from_secs(5));
        drop(lock);
    }
}
