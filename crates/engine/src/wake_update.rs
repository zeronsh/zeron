//! A Cloud machine answers on the latest release. It's woken by a message, and a
//! machine that sleeps most of the time would otherwise run whatever it last had,
//! so before it starts any work its engine looks for a release: newer, it installs
//! it, restarts into it and only then answers.
//!
//! Work is held from the moment the engine assembles, the way an update trial holds
//! it, turns a dying machine left to resume included. Everything else (the token,
//! the rooms) goes on meanwhile, so with nothing to update the check only costs
//! whatever it outlasts them by.
//!
//! The message always gets answered. Each step is bounded, and whatever goes wrong
//! lifts the hold on the version already running; the background updater tries
//! again once it's idle. A machine can't loop: a release it went back from, or
//! couldn't install, isn't tried again from a boot (`zeron_update::BootCheck`).
//!
//! After the restart it's the update trial's turn: the new version holds work until
//! it's proven and commits (`update_trial`), and a new version that fails or hangs
//! gives way to the old one, which boots here again and answers.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use zeron_update::{BootCheck, BootSkipReason, Manifest, PreparedBootUpdate, Updater};

use crate::wake_trace::{self, Stage};
use crate::{EngineCore, doc_host::DocHost, sessions::SessionsEngine, terminals::Terminals};

/// Release metadata is one small file. Longer than this, and the network is the
/// problem: answer now, and let the background updater find it.
const CHECK_LIMIT: Duration = Duration::from_secs(5);
/// Room check, download, verify, unpack, and the new binary answering `--version`,
/// on a disk still arriving. The download floor elsewhere is 1 MiB/s, and a release
/// is about 70 MiB.
const INSTALL_LIMIT: Duration = Duration::from_secs(120);
/// From a restart accepted to this process being stopped. Past it the restart isn't
/// coming, and this version answers.
const RESTART_LIMIT: Duration = Duration::from_secs(30);
/// A new version that hangs holds the message until this, then gives way. Updates
/// nobody is waiting on get longer.
const TRIAL_LIMIT: Duration = Duration::from_secs(5 * 60);

static ARMED: AtomicBool = AtomicBool::new(false);

/// This boot looks for a release before it works. Before assembly: it decides what
/// assembly holds back.
pub fn arm() {
    let armed = crate::runner::session().is_some()
        && !zeron_update::transaction::in_trial()
        && zeron_update::can_update_at_boot();
    ARMED.store(armed, Ordering::SeqCst);
}

pub fn armed() -> bool {
    ARMED.load(Ordering::SeqCst)
}

/// For tests: boot as a machine that can update, on any platform.
#[doc(hidden)]
pub fn force_armed(armed: bool) {
    ARMED.store(armed, Ordering::SeqCst);
}

#[derive(Debug, Clone, PartialEq)]
pub enum Found {
    Current,
    Newer(String),
    /// A release not to try now, and why (the timeline's `updateResult`).
    Skip(&'static str),
}

/// What the gate does, apart from deciding. Separate so its failures can be tested
/// without really replacing an engine; `crates/update` tests the steps themselves.
#[async_trait::async_trait]
pub trait Steps: Send + Sync {
    async fn check(&self) -> anyhow::Result<Found>;
    /// Something this restart would cut off: nothing starts a turn while work is
    /// held, but a person can open a terminal.
    fn busy(&self) -> bool;
    async fn install(&self, limit: Duration) -> anyhow::Result<()>;
    /// Couldn't install it: later boots shouldn't all wait on it.
    fn install_failed(&self, version: &str);
    /// `Ok`: the restart was accepted, and this process is about to stop.
    async fn restart(&self, trial_limit: Duration) -> anyhow::Result<()>;
    fn restart_never_came(&self);
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub check: Duration,
    pub install: Duration,
    pub restart: Duration,
    pub trial: Duration,
}

pub const LIMITS: Limits = Limits {
    check: CHECK_LIMIT,
    install: INSTALL_LIMIT,
    restart: RESTART_LIMIT,
    trial: TRIAL_LIMIT,
};

/// Why work may start on this version (the timeline's `updateResult`). Doesn't
/// return while an update's restart is under way.
pub async fn gate(steps: &dyn Steps, limits: Limits) -> &'static str {
    let found = tokio::time::timeout(limits.check, steps.check()).await;
    wake_trace::mark(Stage::UpdateChecked);
    let version = match found {
        Ok(Ok(Found::Current)) => return "current",
        Ok(Ok(Found::Skip(why))) => return why,
        Ok(Ok(Found::Newer(version))) => version,
        Ok(Err(err)) => {
            tracing::warn!(error = %format!("{err:#}"), "wake update: the check failed; answering on this version");
            return "checkFailed";
        }
        Err(_) => {
            tracing::warn!("wake update: the check took too long; answering on this version");
            return "checkTimedOut";
        }
    };
    if steps.busy() {
        return "busy";
    }
    tracing::info!(%version, "wake update: installing it before answering");
    wake_trace::note_updating(&version);
    wake_trace::announce_stage(Some("updating"));
    // The real step cuts itself off at the limit; this is for one that can't.
    let outer = limits.install + Duration::from_secs(1);
    match tokio::time::timeout(outer, steps.install(limits.install)).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            tracing::warn!(%version, error = %format!("{err:#}"), "wake update: couldn't install it; answering on this version");
            steps.install_failed(&version);
            return "installFailed";
        }
        Err(_) => {
            tracing::warn!(%version, "wake update: installing it took too long; answering on this version");
            steps.install_failed(&version);
            return "installTimedOut";
        }
    }
    if steps.busy() {
        return "busy";
    }
    wake_trace::mark(Stage::UpdateRestart);
    wake_trace::persist();
    if let Err(err) = steps.restart(limits.trial).await {
        tracing::warn!(%version, error = %format!("{err:#}"), "wake update: couldn't restart into it; answering on this version");
        steps.install_failed(&version);
        return "restartFailed";
    }
    tokio::time::sleep(limits.restart).await;
    tracing::warn!(%version, "wake update: the restart never came; answering on this version");
    steps.restart_never_came();
    steps.install_failed(&version);
    "restartNeverCame"
}

struct RealSteps {
    updater: Updater,
    sessions: SessionsEngine,
    terminals: Terminals,
    manifest: Mutex<Option<Manifest>>,
    prepared: Mutex<Option<PreparedBootUpdate>>,
}

fn lock<T>(slot: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    slot.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[async_trait::async_trait]
impl Steps for RealSteps {
    async fn check(&self) -> anyhow::Result<Found> {
        Ok(match self.updater.check_at_boot().await? {
            BootCheck::Current => Found::Current,
            BootCheck::Newer(manifest) => {
                let version = manifest.version.clone();
                *lock(&self.manifest) = Some(manifest);
                Found::Newer(version)
            }
            BootCheck::Skip { version, why } => {
                tracing::info!(%version, ?why, "wake update: not trying this release from a boot");
                Found::Skip(match why {
                    BootSkipReason::WentBack => "wentBack",
                    BootSkipReason::Failed => "failedBefore",
                    BootSkipReason::TriedRecently => "triedRecently",
                    BootSkipReason::InProgress => "inProgress",
                })
            }
        })
    }

    fn busy(&self) -> bool {
        self.sessions.any_active() || self.terminals.any_open()
    }

    async fn install(&self, limit: Duration) -> anyhow::Result<()> {
        let manifest = lock(&self.manifest)
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no release to install"))?;
        let prepared = self.updater.install_at_boot(&manifest, limit).await?;
        *lock(&self.prepared) = Some(prepared);
        Ok(())
    }

    fn install_failed(&self, version: &str) {
        zeron_update::note_boot_update_failed(version);
    }

    async fn restart(&self, trial_limit: Duration) -> anyhow::Result<()> {
        let prepared = lock(&self.prepared)
            .take()
            .ok_or_else(|| anyhow::anyhow!("nothing installed"))?;
        self.updater.restart_at_boot(prepared, trial_limit).await
    }

    fn restart_never_came(&self) {
        self.updater.boot_restart_never_came();
    }
}

/// [`gate`] over `core`, then let work start.
pub async fn gate_then_release(
    core: &EngineCore,
    steps: &dyn Steps,
    limits: Limits,
) -> &'static str {
    let result = gate(steps, limits).await;
    release(
        core.doc_host.clone(),
        core.sessions.clone(),
        core.updater(),
        result,
    );
    result
}

/// Look for a release, then let work start, unless a restart gets there first.
pub(crate) fn start(core: &EngineCore) {
    if !armed() {
        return;
    }
    let doc_host = core.doc_host.clone();
    let sessions = core.sessions.clone();
    let Some(updater) = core.updater() else {
        release(doc_host, sessions, None, "noUpdater");
        return;
    };
    updater.hold_background_updates(true);
    let steps = RealSteps {
        updater: updater.clone(),
        sessions: sessions.clone(),
        terminals: core.terminals.clone(),
        manifest: Mutex::new(None),
        prepared: Mutex::new(None),
    };
    tokio::spawn(async move {
        let result = gate(&steps, LIMITS).await;
        tracing::info!(result, "wake update: work may start");
        release(doc_host, sessions, Some(updater), result);
    });
}

/// What every other start does during assembly, then what was waiting.
fn release(
    doc_host: DocHost,
    sessions: SessionsEngine,
    updater: Option<Updater>,
    result: &'static str,
) {
    finished(result);
    if let Some(updater) = updater {
        updater.hold_background_updates(false);
    }
    tokio::spawn(async move {
        match tokio::task::spawn_blocking(move || sessions.recover_stale()).await {
            Ok(Ok(0)) => {}
            Ok(Ok(recovered)) => {
                tracing::info!(recovered, "stale sessions recovered after the update check")
            }
            Ok(Err(err)) => tracing::error!(error = %err, "stale-session recovery failed"),
            Err(err) => tracing::error!(error = %err, "stale-session recovery panicked"),
        }
        doc_host.lift_quiesce();
    });
}

/// The wait for an update is over, on whichever version: the timeline says so, and
/// a machine that said it was updating stops saying it.
pub(crate) fn finished(result: &'static str) {
    wake_trace::note_result(result);
    wake_trace::mark(Stage::Released);
    wake_trace::forget_persisted();
    if wake_trace::was_updating() {
        wake_trace::announce_stage(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct Scripted {
        check: Mutex<Option<anyhow::Result<Found>>>,
        check_hangs: bool,
        busy: AtomicBool,
        install_fails: bool,
        install_hangs: bool,
        restart_fails: bool,
        installs: AtomicUsize,
        restarts: AtomicUsize,
        failures_noted: Mutex<Vec<String>>,
        never_came: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl Steps for Scripted {
        async fn check(&self) -> anyhow::Result<Found> {
            if self.check_hangs {
                std::future::pending::<()>().await;
            }
            lock(&self.check).take().unwrap()
        }
        fn busy(&self) -> bool {
            self.busy.load(Ordering::SeqCst)
        }
        async fn install(&self, _limit: Duration) -> anyhow::Result<()> {
            self.installs.fetch_add(1, Ordering::SeqCst);
            if self.install_hangs {
                std::future::pending::<()>().await;
            }
            if self.install_fails {
                anyhow::bail!("HTTP 500");
            }
            Ok(())
        }
        fn install_failed(&self, version: &str) {
            lock(&self.failures_noted).push(version.to_owned());
        }
        async fn restart(&self, _trial_limit: Duration) -> anyhow::Result<()> {
            self.restarts.fetch_add(1, Ordering::SeqCst);
            if self.restart_fails {
                anyhow::bail!("sudo: a password is required");
            }
            Ok(())
        }
        fn restart_never_came(&self) {
            self.never_came.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn found(found: Found) -> Mutex<Option<anyhow::Result<Found>>> {
        Mutex::new(Some(Ok(found)))
    }

    const FAST: Limits = Limits {
        check: Duration::from_millis(50),
        install: Duration::from_millis(50),
        restart: Duration::from_millis(50),
        trial: Duration::from_secs(1),
    };

    #[tokio::test]
    async fn no_update_lets_work_start_at_once_and_installs_nothing() {
        let steps = Scripted {
            check: found(Found::Current),
            ..Scripted::default()
        };
        let started = std::time::Instant::now();
        assert_eq!(gate(&steps, LIMITS).await, "current");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(steps.installs.load(Ordering::SeqCst), 0);
        assert_eq!(steps.restarts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn an_update_installs_and_restarts_and_work_stays_held_until_the_process_stops() {
        let steps = Scripted {
            check: found(Found::Newer("9.9.9".into())),
            ..Scripted::default()
        };
        // A long restart wait: this never finishes, as the process would be stopped.
        let limits = Limits {
            restart: Duration::from_secs(600),
            ..FAST
        };
        let waited = tokio::time::timeout(Duration::from_millis(200), gate(&steps, limits)).await;
        assert!(
            waited.is_err(),
            "work must stay held while the restart is on its way"
        );
        assert_eq!(steps.installs.load(Ordering::SeqCst), 1);
        assert_eq!(steps.restarts.load(Ordering::SeqCst), 1);
        assert!(lock(&steps.failures_noted).is_empty());
    }

    #[tokio::test]
    async fn a_restart_that_never_comes_ends_with_this_version_answering() {
        let steps = Scripted {
            check: found(Found::Newer("9.9.9".into())),
            ..Scripted::default()
        };
        assert_eq!(gate(&steps, FAST).await, "restartNeverCame");
        assert_eq!(steps.never_came.load(Ordering::SeqCst), 1);
        assert_eq!(*lock(&steps.failures_noted), ["9.9.9"]);
    }

    #[tokio::test]
    async fn a_check_that_hangs_gives_way_at_its_limit() {
        let steps = Scripted {
            check_hangs: true,
            ..Scripted::default()
        };
        let started = std::time::Instant::now();
        assert_eq!(gate(&steps, FAST).await, "checkTimedOut");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(steps.installs.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_check_that_fails_gives_way_at_once_and_is_tried_again_later() {
        let steps = Scripted {
            check: Mutex::new(Some(Err(anyhow::anyhow!("connection refused")))),
            ..Scripted::default()
        };
        assert_eq!(gate(&steps, LIMITS).await, "checkFailed");
        // The next wake looks again: a check costs little.
        assert!(lock(&steps.failures_noted).is_empty());
    }

    #[tokio::test]
    async fn a_failed_download_ends_with_this_version_answering_and_no_restart() {
        let steps = Scripted {
            check: found(Found::Newer("9.9.9".into())),
            install_fails: true,
            ..Scripted::default()
        };
        assert_eq!(gate(&steps, LIMITS).await, "installFailed");
        assert_eq!(steps.restarts.load(Ordering::SeqCst), 0);
        assert_eq!(*lock(&steps.failures_noted), ["9.9.9"]);
    }

    #[tokio::test]
    async fn a_download_that_hangs_gives_way_at_its_limit() {
        let steps = Scripted {
            check: found(Found::Newer("9.9.9".into())),
            install_hangs: true,
            ..Scripted::default()
        };
        let started = std::time::Instant::now();
        assert_eq!(gate(&steps, FAST).await, "installTimedOut");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(steps.restarts.load(Ordering::SeqCst), 0);
        assert_eq!(*lock(&steps.failures_noted), ["9.9.9"]);
    }

    #[tokio::test]
    async fn a_refused_restart_ends_with_this_version_answering() {
        let steps = Scripted {
            check: found(Found::Newer("9.9.9".into())),
            restart_fails: true,
            ..Scripted::default()
        };
        assert_eq!(gate(&steps, LIMITS).await, "restartFailed");
        assert_eq!(*lock(&steps.failures_noted), ["9.9.9"]);
    }

    /// What the old version sees booting after a bad new version's rollback: it
    /// doesn't try again, so it answers, and the machine doesn't loop.
    #[tokio::test]
    async fn after_going_back_from_a_release_it_answers_without_trying_again() {
        let steps = Scripted {
            check: found(Found::Skip("wentBack")),
            ..Scripted::default()
        };
        assert_eq!(gate(&steps, LIMITS).await, "wentBack");
        assert_eq!(steps.installs.load(Ordering::SeqCst), 0);
        assert_eq!(steps.restarts.load(Ordering::SeqCst), 0);
    }

    /// Something a restart would cut off is never cut off, before or after the
    /// download, and isn't held against the release.
    #[tokio::test]
    async fn nothing_running_is_restarted_under() {
        let steps = Scripted {
            check: found(Found::Newer("9.9.9".into())),
            ..Scripted::default()
        };
        steps.busy.store(true, Ordering::SeqCst);
        assert_eq!(gate(&steps, LIMITS).await, "busy");
        assert_eq!(steps.installs.load(Ordering::SeqCst), 0);
        assert_eq!(steps.restarts.load(Ordering::SeqCst), 0);
        assert!(lock(&steps.failures_noted).is_empty());
    }
}
