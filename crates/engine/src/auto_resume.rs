//! Auto-resume: turns the provider's usage limit stopped continue by
//! themselves once the limit resets — main chats, side chats and the worker
//! chats agents created alike, since every one of them is a chat this engine
//! hosts.
//!
//! The stop itself is recorded by the run as it ends, on the chat's session
//! row ([`zeron_proto::AutoResume`], see `sessions::limits`), so every device
//! reads it where it reads the run's status. This service owns what follows:
//!
//! - **When.** The reset time the provider gave (Claude's `resetsAt`,
//!   Codex's exhausted window, the error text), else the live account's usage
//!   windows ([`AgentAccounts::active_usage`]), else a check on a growing
//!   backoff ([`CHECK_BACKOFF`]).
//! - **Confirm.** At that time the account's usage is probed again (within
//!   the probe backoff); a window still exhausted moves the resume to its
//!   reset instead of hitting the provider with a turn it will refuse.
//! - **Resume.** Every stop of that provider continues together, side chats
//!   before their parents, so a parent that picks up where it left off finds
//!   its children working again ([`SessionsEngine::resume_limited`]).
//! - **Cancel.** Another turn in the chat (`sessions::limits::turn_started`),
//!   a Stop, archiving or deleting the chat, or the toggle drop it.
//! - **Survive restarts.** Waiting stops are saved to `auto-resumes.json` in
//!   the profile store and restored at boot; one whose reset passed while the
//!   engine was down resumes right away.
//!
//! Whether new stops resume by default is the device's setting
//! (`auto-resume.json` in the data dir, on unless turned off).

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use zeron_proto::auto_resume::{MAX_ATTEMPTS, check_backoff};
use zeron_proto::{
    AgentAccount, AutoResume, AutoResumePhase, AutoResumeSettings, Chat, HarnessId, Session,
};

use crate::EngineError;
use crate::agent_accounts::AgentAccounts;
use crate::sessions::{LimitResume, SessionsEngine};
use crate::workspace_host::WorkspaceHost;

/// How long after a window's reset the resume waits, so the provider has
/// certainly rolled the window over (a time read from text is to the minute).
const RESET_MARGIN: Duration = Duration::seconds(60);

/// The scheduler looks again at least this often (it probes nothing unless
/// a stop is due).
const IDLE_TICK: std::time::Duration = std::time::Duration::from_secs(5 * 60);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone)]
pub struct AutoResumer {
    inner: Arc<Inner>,
}

struct Inner {
    settings_path: PathBuf,
    /// The device setting, shared with the sessions engine (the default a
    /// new stop's toggle starts from).
    enabled: Arc<AtomicBool>,
    /// Serializes setting writes.
    settings_write: Mutex<()>,
    saved_path: PathBuf,
    /// The stops last written to `saved_path`.
    saved: Mutex<Option<Vec<Session>>>,
    /// [`RESET_MARGIN`]; tests shorten it.
    reset_margin: Mutex<Duration>,
    sessions: SessionsEngine,
    workspace: WorkspaceHost,
    accounts: AgentAccounts,
    wake: Notify,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SavedStops {
    /// The rows of the stops still waiting, as they were.
    sessions: Vec<Session>,
}

impl AutoResumer {
    /// Read the device setting from `data_dir` and the saved stops from
    /// `store_root`, and put those stops back on their rows. Starts nothing:
    /// see [`Self::start`].
    pub fn open(
        data_dir: &Path,
        store_root: &Path,
        sessions: SessionsEngine,
        workspace: WorkspaceHost,
        accounts: AgentAccounts,
    ) -> Self {
        let settings_path = data_dir.join("auto-resume.json");
        let settings: AutoResumeSettings = std::fs::read(&settings_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let enabled = Arc::new(AtomicBool::new(settings.enabled));
        sessions.set_auto_resume_default(enabled.clone());
        let saved_path = store_root.join("auto-resumes.json");
        let saved: SavedStops = std::fs::read(&saved_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        for mut row in saved.sessions {
            // Deleted or archived while the engine was down: over.
            if !workspace
                .chat(&row.chat_id)
                .ok()
                .flatten()
                .is_some_and(|chat| !chat.archived)
            {
                continue;
            }
            // A continuation the engine went down starting: start it again
            // (dispatch finds out if it got going after all).
            if let Some(resume) = row.auto_resume.as_mut()
                && resume.phase == AutoResumePhase::Resuming
            {
                resume.phase = AutoResumePhase::Waiting;
            }
            sessions.restore_auto_resume(row);
        }
        Self {
            inner: Arc::new(Inner {
                settings_path,
                enabled,
                settings_write: Mutex::new(()),
                saved_path,
                saved: Mutex::new(None),
                reset_margin: Mutex::new(RESET_MARGIN),
                sessions,
                workspace,
                accounts,
                wake: Notify::new(),
                task: Mutex::new(None),
            }),
        }
    }

    /// Run the scheduler until [`Self::shutdown`].
    pub fn start(&self) {
        let inner = self.inner.clone();
        let task = tokio::spawn(async move { inner.run().await });
        if let Some(previous) = lock(&self.inner.task).replace(task) {
            previous.abort();
        }
    }

    /// Stop the scheduler and save the stops as they are.
    pub fn shutdown(&self) {
        if let Some(task) = lock(&self.inner.task).take() {
            task.abort();
        }
        self.inner.save();
    }

    /// Wait `margin` after a reset instead of [`RESET_MARGIN`] (tests).
    #[doc(hidden)]
    pub fn set_reset_margin(&self, margin: Duration) {
        *lock(&self.inner.reset_margin) = margin;
        self.inner.wake.notify_one();
    }

    pub fn settings(&self) -> AutoResumeSettings {
        AutoResumeSettings {
            enabled: self.inner.enabled.load(Ordering::Relaxed),
        }
    }

    /// Change the device setting. It decides new stops' toggles, and flips
    /// the toggle of every stop waiting now: "off" means nothing resumes.
    pub fn set_enabled(&self, enabled: bool) -> Result<AutoResumeSettings, EngineError> {
        {
            let _write = lock(&self.inner.settings_write);
            let parent = self
                .inner
                .settings_path
                .parent()
                .ok_or_else(|| EngineError::Other("settings have no folder".into()))?;
            std::fs::create_dir_all(parent)?;
            let mut temp = tempfile::NamedTempFile::new_in(parent)?;
            serde_json::to_writer_pretty(&mut temp, &AutoResumeSettings { enabled })
                .map_err(|e| EngineError::Other(e.to_string()))?;
            temp.write_all(b"\n")?;
            temp.persist(&self.inner.settings_path)
                .map_err(|e| EngineError::Io(e.error))?;
            self.inner.enabled.store(enabled, Ordering::Relaxed);
        }
        for row in self.inner.sessions.auto_resumes() {
            self.inner
                .sessions
                .update_auto_resume(&row.chat_id, |state| {
                    if let Some(resume) = state
                        .as_mut()
                        .filter(|resume| resume.phase == AutoResumePhase::Waiting)
                    {
                        resume.enabled = enabled;
                    }
                });
        }
        self.inner.save();
        self.inner.wake.notify_one();
        Ok(self.settings())
    }

    /// Flip one chat's stop. Switching on a stop the host gave up on starts
    /// it over. Returns its state after (`None`: the chat has no stop).
    pub fn set_chat(&self, chat_id: &str, enabled: bool) -> Option<AutoResume> {
        let state = self.inner.sessions.update_auto_resume(chat_id, |state| {
            if let Some(resume) = state.as_mut() {
                resume.enabled = enabled;
                if enabled && resume.phase == AutoResumePhase::GaveUp {
                    resume.phase = AutoResumePhase::Waiting;
                    resume.attempt = 0;
                    resume.resume_at = None;
                }
            }
        });
        self.inner.save();
        self.inner.wake.notify_one();
        state
    }
}

/// What the scheduler remembers between passes (not saved: a restart
/// re-derives it).
#[derive(Default)]
struct Schedule {
    /// Per chat whose stop has no reset time: when to check next.
    checks: HashMap<String, DateTime<Utc>>,
    /// Chats seen in the registry, so a chat missing from it was deleted
    /// rather than not loaded yet.
    known: HashSet<String>,
}

/// One stop the scheduler acts on.
struct Due {
    chat_id: String,
    entry_id: String,
    attempt: u32,
    /// A side chat: resumed before the top-level chats.
    child: bool,
    stopped_at: DateTime<Utc>,
}

impl Inner {
    async fn run(self: Arc<Self>) {
        let mut sessions = self.sessions.watch_sessions();
        let mut chats = self.workspace.watch_chats();
        let mut schedule = Schedule::default();
        loop {
            let next = self.pass(&mut schedule).await;
            let sleep = next
                .map(|at| (at - Utc::now()).to_std().unwrap_or_default())
                .map_or(IDLE_TICK, |wait| wait.min(IDLE_TICK));
            tokio::select! {
                changed = sessions.changed() => if changed.is_err() { break },
                changed = chats.changed() => if changed.is_err() { break },
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(sleep) => {}
            }
        }
    }

    /// One look at every stop: drop the ones whose chat went away, save
    /// what waits, find out when the new ones can resume, and resume the due
    /// ones. Returns when to look again.
    async fn pass(&self, schedule: &mut Schedule) -> Option<DateTime<Utc>> {
        let chats: HashMap<String, Chat> = self
            .workspace
            .watch_chats()
            .borrow()
            .iter()
            .map(|chat| (chat.id.clone(), chat.clone()))
            .collect();
        for row in self.sessions.auto_resumes() {
            let gone = match chats.get(&row.chat_id) {
                Some(chat) => chat.archived,
                None => schedule.known.contains(&row.chat_id),
            };
            if gone && let Some(resume) = &row.auto_resume {
                self.sessions
                    .cancel_auto_resume(&row.chat_id, Some(&resume.entry_id));
            }
        }
        schedule.known.extend(chats.keys().cloned());
        self.save();

        let now = Utc::now();
        let rows = self.sessions.auto_resumes();
        let waiting: Vec<(&Session, &AutoResume)> = rows
            .iter()
            .filter_map(|row| Some((row, row.auto_resume.as_ref()?)))
            .filter(|(_, resume)| resume.enabled && resume.phase == AutoResumePhase::Waiting)
            .collect();
        schedule
            .checks
            .retain(|chat, _| waiting.iter().any(|(row, _)| &row.chat_id == chat));

        // Per provider: the stops to learn a reset time for, and the due ones.
        let mut unknown: HashMap<Option<HarnessId>, Vec<&Session>> = HashMap::new();
        let mut due: HashMap<Option<HarnessId>, Vec<Due>> = HashMap::new();
        let mut next: Option<DateTime<Utc>> = None;
        for (row, resume) in &waiting {
            // Resumed only in a chat that is there and not archived (one not
            // loaded yet waits for it).
            let Some(chat) = chats.get(&row.chat_id).filter(|chat| !chat.archived) else {
                continue;
            };
            let harness = chat.config.as_ref().map(|c| c.harness);
            let at = match (resume.resume_at, schedule.checks.get(&row.chat_id)) {
                (Some(reset), _) => reset + *lock(&self.reset_margin),
                (None, Some(check)) => *check,
                (None, None) => {
                    unknown.entry(harness).or_default().push(row);
                    continue;
                }
            };
            if at <= now {
                due.entry(harness).or_default().push(Due {
                    chat_id: row.chat_id.clone(),
                    entry_id: resume.entry_id.clone(),
                    attempt: resume.attempt,
                    child: chat.parent_chat_id.is_some(),
                    stopped_at: resume.stopped_at,
                });
            } else {
                next = Some(next.map_or(at, |next| next.min(at)));
            }
        }

        // New stops without a reset time: the account's windows may know.
        for (harness, rows) in unknown {
            let until = match harness {
                Some(harness) => self.exhausted_until(harness, now).await,
                None => None,
            };
            for row in rows {
                let Some(stop) = row.auto_resume.as_ref() else {
                    continue;
                };
                match until {
                    Some(reset) => {
                        set_resume_at(&self.sessions, &row.chat_id, &stop.entry_id, reset);
                    }
                    None => {
                        let resume = row.auto_resume.as_ref();
                        let check = resume.map_or(now, |resume| {
                            (resume.stopped_at + check_backoff(resume.attempt)).max(now)
                        });
                        schedule.checks.insert(row.chat_id.clone(), check);
                    }
                }
            }
            // Revisited at once: the rows changed.
            next = Some(now);
        }

        for (harness, mut stops) in due {
            // Confirm before resuming: a window still exhausted moves them.
            if let Some(harness) = harness
                && let Some(reset) = self.exhausted_until(harness, now).await
            {
                for stop in &stops {
                    schedule.checks.remove(&stop.chat_id);
                    set_resume_at(&self.sessions, &stop.chat_id, &stop.entry_id, reset);
                }
                next = Some(next.map_or(reset, |next| next.min(reset)));
                continue;
            }
            // Side chats first: their parents pick up waiting on them.
            stops.sort_by_key(|stop| (!stop.child, stop.stopped_at));
            for stop in stops {
                schedule.checks.remove(&stop.chat_id);
                match self
                    .sessions
                    .resume_limited(&stop.chat_id, &stop.entry_id)
                    .await
                {
                    Ok(LimitResume::Started | LimitResume::NotNow) => {}
                    Ok(LimitResume::MovedOn) => self
                        .sessions
                        .cancel_auto_resume(&stop.chat_id, Some(&stop.entry_id)),
                    Err(err) => {
                        // Counts like a resume the limit took straight back.
                        let attempt = stop.attempt.saturating_add(1);
                        tracing::warn!(chat = %stop.chat_id, error = %err, attempt,
                            "auto-resume after the usage limit failed");
                        self.sessions.update_auto_resume(&stop.chat_id, |state| {
                            if let Some(resume) = state.as_mut().filter(|resume| {
                                resume.entry_id == stop.entry_id
                                    && resume.phase == AutoResumePhase::Waiting
                            }) {
                                resume.resume_at = None;
                                resume.attempt = attempt;
                                if attempt >= MAX_ATTEMPTS {
                                    resume.phase = AutoResumePhase::GaveUp;
                                }
                            }
                        });
                        let retry = now + check_backoff(attempt);
                        schedule.checks.insert(stop.chat_id.clone(), retry);
                        next = Some(next.map_or(retry, |next| next.min(retry)));
                    }
                }
            }
        }
        next
    }

    /// When `harness`'s live account can run again, if one of its windows is
    /// still used up: the latest reset among them. `None` when nothing says
    /// it is still limited (no account, no windows, the probe failed and the
    /// last windows it had have reset meanwhile).
    async fn exhausted_until(
        &self,
        harness: HarnessId,
        now: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        match self.accounts.active_usage(harness, true).await {
            Ok(account) => account.as_ref().and_then(|account| exhausted(account, now)),
            Err(err) => {
                tracing::debug!(?harness, error = %err, "auto-resume usage check failed");
                None
            }
        }
    }

    /// Save the stops not yet resumed, when they changed.
    fn save(&self) {
        let mut saved = lock(&self.saved);
        let mut rows: Vec<Session> = self
            .sessions
            .auto_resumes()
            .into_iter()
            .filter(|row| {
                row.auto_resume
                    .as_ref()
                    .is_some_and(|resume| resume.phase != AutoResumePhase::Resumed)
            })
            .collect();
        rows.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
        // Freshness stamps alone are no change worth a write.
        let key = |rows: &[Session]| {
            rows.iter()
                .map(|row| (row.chat_id.clone(), row.auto_resume.clone()))
                .collect::<Vec<_>>()
        };
        if saved
            .as_deref()
            .is_some_and(|saved| key(saved) == key(&rows))
        {
            return;
        }
        let written = (|| -> Result<(), EngineError> {
            let parent = self
                .saved_path
                .parent()
                .ok_or_else(|| EngineError::Other("store has no folder".into()))?;
            std::fs::create_dir_all(parent)?;
            let mut temp = tempfile::NamedTempFile::new_in(parent)?;
            serde_json::to_writer(
                &mut temp,
                &SavedStops {
                    sessions: rows.clone(),
                },
            )
            .map_err(|e| EngineError::Other(e.to_string()))?;
            temp.persist(&self.saved_path)
                .map_err(|e| EngineError::Io(e.error))?;
            Ok(())
        })();
        match written {
            Ok(()) => *saved = Some(rows),
            Err(err) => tracing::warn!(error = %err, "saving auto-resumes failed"),
        }
    }
}

/// Move stop `entry_id`'s resume to `reset` (no sooner than its attempt's
/// backoff after the stop), if it still waits.
fn set_resume_at(sessions: &SessionsEngine, chat_id: &str, entry_id: &str, reset: DateTime<Utc>) {
    sessions.update_auto_resume(chat_id, |state| {
        if let Some(resume) = state.as_mut().filter(|resume| {
            resume.entry_id == entry_id && resume.phase == AutoResumePhase::Waiting
        }) {
            resume.resume_at = Some(if resume.attempt > 0 {
                reset.max(resume.stopped_at + check_backoff(resume.attempt))
            } else {
                reset
            });
        }
    });
}

/// The latest reset among `account`'s used-up windows still ahead of `now`.
fn exhausted(account: &AgentAccount, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    account
        .usage_windows
        .iter()
        .filter(|window| window.used_fraction >= 1.0)
        .filter_map(|window| window.resets_at)
        .filter(|reset| *reset > now)
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::AgentUsageWindow;

    fn account(windows: Vec<AgentUsageWindow>) -> AgentAccount {
        serde_json::from_value(serde_json::json!({
            "id": "slot",
            "harness": "claude-code",
            "active": true,
            "usageWindows": windows,
        }))
        .expect("account")
    }

    fn window(used: f32, resets_at: Option<DateTime<Utc>>) -> AgentUsageWindow {
        AgentUsageWindow {
            label: "Session".into(),
            used_fraction: used,
            resets_at,
        }
    }

    #[test]
    fn an_account_is_limited_until_its_latest_used_up_window_resets() {
        let now = Utc::now();
        let soon = now + Duration::hours(1);
        let later = now + Duration::days(2);
        assert_eq!(
            exhausted(
                &account(vec![window(1.0, Some(soon)), window(1.2, Some(later))]),
                now
            ),
            Some(later)
        );
        // Windows with room left don't hold it back.
        assert_eq!(
            exhausted(
                &account(vec![window(1.0, Some(soon)), window(0.4, Some(later))]),
                now
            ),
            Some(soon)
        );
        // Stale windows whose reset passed, or with no time: nothing known.
        assert_eq!(
            exhausted(
                &account(vec![window(1.0, Some(now - Duration::minutes(1)))]),
                now
            ),
            None
        );
        assert_eq!(exhausted(&account(vec![window(1.0, None)]), now), None);
        assert_eq!(exhausted(&account(Vec::new()), now), None);
    }

    #[test]
    fn checks_back_off_with_each_stop_in_a_row() {
        assert_eq!(check_backoff(0), Duration::minutes(5));
        assert_eq!(check_backoff(1), Duration::minutes(15));
        assert_eq!(check_backoff(3), Duration::minutes(60));
        assert_eq!(check_backoff(40), Duration::minutes(60));
    }
}
