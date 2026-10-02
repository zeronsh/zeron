//! Engine-owned self-update operations (`engine-updates-v1`).
//!
//! A client only asks; this device decides. An accepted operation belongs to
//! the engine: it survives the requesting view, the relay, and — through a
//! small record on disk — the restart that completes it, so the restarted
//! engine can report whether it actually runs the target version.
//!
//! Restart safety has two halves. Waiting until nothing is running is not
//! enough on its own: a run could be admitted between observing idle and the
//! swap. [`RestartGate`] closes that window — new runs and terminals take an
//! admission ticket, and the gate only closes while no ticket is outstanding
//! and the engine is idle.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedRwLockReadGuard, RwLock, watch};
use tokio_util::sync::CancellationToken;
use zeron_proto::{
    EngineUpdateAck, EngineUpdateOperation, EngineUpdatePhase, EngineUpdateState,
    EngineUpdateSupport,
};

use crate::now_ms;

const IDLE_POLL: Duration = Duration::from_secs(2);
/// Lets the `Restarting` frame reach relayed clients before the service
/// manager stops this process.
const RESTART_GRACE: Duration = Duration::from_millis(800);
/// A supervisor that reports success but never stops this process must not
/// leave admission closed forever.
const RESTART_WATCHDOG: Duration = Duration::from_secs(90);
/// How often to look for a newer binary someone else installed (the desktop
/// app swapping its bundle, `zeron update` flipping the symlink).
const INSTALLED_POLL: Duration = Duration::from_secs(60);
const RECORD_FILE: &str = "engine-update-operation.json";
const RESTARTING_MESSAGE: &str = "Zeron is restarting to finish an update. Try again in a moment.";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Admission control for work a restart would interrupt.
#[derive(Clone, Default)]
pub struct RestartGate {
    inner: Arc<GateInner>,
}

#[derive(Default)]
struct GateInner {
    tickets: Arc<RwLock<()>>,
    closed: std::sync::atomic::AtomicBool,
}

/// Held by new work until it is visible to the idle check.
pub struct Admission(#[allow(dead_code)] OwnedRwLockReadGuard<()>);

impl RestartGate {
    fn closed(&self) -> bool {
        self.inner.closed.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Admit a run. Hold the ticket until the run is registered as active.
    pub async fn admit(&self) -> Result<Admission, String> {
        let ticket = self.inner.tickets.clone().read_owned().await;
        if self.closed() {
            return Err(RESTARTING_MESSAGE.into());
        }
        Ok(Admission(ticket))
    }

    /// Synchronous admission (terminal open). Only contends with the brief
    /// moment in which a restart evaluates idleness.
    pub fn try_admit(&self) -> Result<Admission, String> {
        let ticket = self
            .inner
            .tickets
            .clone()
            .try_read_owned()
            .map_err(|_| RESTARTING_MESSAGE.to_string())?;
        if self.closed() {
            return Err(RESTARTING_MESSAGE.into());
        }
        Ok(Admission(ticket))
    }

    /// Close admission if, with no admission in flight, `idle` holds. Never
    /// waits for tickets: a queued writer would stall every new run behind a
    /// slow dispatch, so a busy instant simply tries again later.
    pub(crate) fn close_if(&self, idle: &(dyn Fn() -> bool + Send + Sync)) -> bool {
        if !idle() {
            return false;
        }
        let Ok(_exclusive) = self.inner.tickets.try_write() else {
            return false;
        };
        if !idle() {
            return false;
        }
        self.inner
            .closed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        true
    }

    fn reopen(&self) {
        self.inner
            .closed
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// A desktop restart reservation. Failed installation or a dropped UI task
/// reopens admission; successful installation keeps it closed until shutdown.
pub struct DesktopRestartPermit(Option<RestartGate>);
impl DesktopRestartPermit {
    pub fn commit(mut self) {
        self.0.take();
    }
}
impl Drop for DesktopRestartPermit {
    fn drop(&mut self) {
        if let Some(gate) = self.0.take() {
            gate.reopen();
        }
    }
}

/// The installation mechanics, separated so the lifecycle can be exercised
/// without downloading or restarting anything.
#[async_trait]
pub trait EngineInstaller: Send + Sync {
    fn support(&self) -> EngineUpdateSupport;
    /// Whether a service manager runs this process, so [`Self::restart`]
    /// brings back whatever is installed. Independent of who installs: a
    /// launchd daemon inside the app bundle cannot install, but can restart
    /// into the bundle the desktop app swapped.
    fn supervised(&self) -> bool {
        matches!(self.support(), EngineUpdateSupport::Managed)
    }
    fn running_version(&self) -> String;
    fn installed_version(&self) -> Option<String>;
    fn installation_identity(&self) -> String {
        format!("{:?}:{:?}", self.support(), self.installed_version())
    }
    /// The newest release the feed offers.
    async fn latest(&self) -> Result<String, String>;
    /// Download and verify `version` without touching the installation.
    async fn stage(&self, version: &str) -> Result<(), String>;
    /// Atomically make the staged `version` the installed one.
    fn apply(&self, version: &str) -> Result<(), String>;
    /// Ask the supervisor to restart this engine.
    fn restart(&self) -> Result<(), String>;
}

/// The real installer: `crates/update`'s managed layout and service restart.
pub struct ManagedInstaller {
    edge_url: String,
    /// The manifest `latest` read, so staging verifies that release's checksums.
    manifest: Mutex<Option<zeron_update::Manifest>>,
}

impl ManagedInstaller {
    pub fn new(edge_url: String) -> Self {
        Self {
            edge_url,
            manifest: Mutex::new(None),
        }
    }

    fn app_root() -> Result<PathBuf, String> {
        match zeron_update::engine_install_support() {
            zeron_update::EngineInstallSupport::Managed { app_root }
            | zeron_update::EngineInstallSupport::ManualRestart { app_root } => Ok(app_root),
            zeron_update::EngineInstallSupport::Unsupported(reason) => Err(reason),
        }
    }
}

#[async_trait]
impl EngineInstaller for ManagedInstaller {
    fn support(&self) -> EngineUpdateSupport {
        match zeron_update::engine_install_support() {
            zeron_update::EngineInstallSupport::Managed { .. } => EngineUpdateSupport::Managed,
            zeron_update::EngineInstallSupport::ManualRestart { .. } => {
                EngineUpdateSupport::ManualRestart {
                    reason: "This engine was started by hand, so Zeron can install the update \
                             but can't restart it. Restart it where it was started."
                        .into(),
                }
            }
            zeron_update::EngineInstallSupport::Unsupported(reason) => {
                EngineUpdateSupport::Unsupported { reason }
            }
        }
    }

    fn supervised(&self) -> bool {
        zeron_update::running_as_installed_service()
    }

    fn running_version(&self) -> String {
        zeron_update::current_version().to_owned()
    }

    fn installed_version(&self) -> Option<String> {
        zeron_update::installed_version(&zeron_update::detect_install())
    }

    fn installation_identity(&self) -> String {
        match Self::app_root() {
            Ok(root) => {
                let current = root.join("current");
                format!(
                    "{}:{:?}:{:?}",
                    root.display(),
                    current.canonicalize().ok(),
                    current
                        .symlink_metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                )
            }
            Err(error) => error,
        }
    }

    async fn latest(&self) -> Result<String, String> {
        let manifest = zeron_update::fetch_latest(&self.edge_url)
            .await
            .map_err(|error| format!("{error:#}"))?;
        let version = manifest.version.clone();
        *lock(&self.manifest) = Some(manifest);
        Ok(version)
    }

    async fn stage(&self, version: &str) -> Result<(), String> {
        let manifest = lock(&self.manifest)
            .clone()
            .filter(|manifest| manifest.version == version)
            .ok_or_else(|| format!("release {version} is no longer offered"))?;
        let app_root = Self::app_root()?;
        zeron_update::stage_headless(&self.edge_url, &manifest, &app_root)
            .await
            .map(drop)
            .map_err(|error| format!("{error:#}"))
    }

    fn apply(&self, version: &str) -> Result<(), String> {
        zeron_update::apply_headless(&Self::app_root()?, version)
            .map_err(|error| format!("{error:#}"))
    }

    fn restart(&self) -> Result<(), String> {
        zeron_update::restart_service().map_err(|error| format!("{error:#}"))
    }
}

/// What survives the restart: enough to judge the outcome afterwards.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OperationRecord {
    operation: EngineUpdateOperation,
}

pub type IdleCheck = Arc<dyn Fn() -> bool + Send + Sync>;

struct Inner {
    installer: Arc<dyn EngineInstaller>,
    updater: Option<zeron_update::Updater>,
    gate: RestartGate,
    idle: IdleCheck,
    idle_poll: Duration,
    record_path: PathBuf,
    state_tx: watch::Sender<EngineUpdateState>,
    /// Cancellation of the operation in flight. Guarded together with its
    /// phase: the apply boundary and a cancel request serialize here.
    control: Mutex<Option<CancellationToken>>,
    shutdown: CancellationToken,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    operation_gate: tokio::sync::Mutex<()>,
}

/// Cloneable service behind the engine-update RPCs.
#[derive(Clone)]
pub struct EngineUpdates {
    inner: Arc<Inner>,
}

impl EngineUpdates {
    pub fn new(
        data_dir: &Path,
        installer: Arc<dyn EngineInstaller>,
        updater: Option<zeron_update::Updater>,
        gate: RestartGate,
        idle: IdleCheck,
    ) -> Self {
        let record_path = data_dir.join(RECORD_FILE);
        let operation = Self::recover(&record_path, installer.as_ref());
        let status = updater
            .as_ref()
            .map(|updater| updater.watch().borrow().clone());
        let state = EngineUpdateState {
            running_version: installer.running_version(),
            installed_version: installer.installed_version(),
            latest_version: status.as_ref().and_then(|s| s.latest_version.clone()),
            checked_at: status.as_ref().and_then(|s| s.checked_at),
            check_error: status.as_ref().and_then(|s| s.error.clone()),
            support: installer.support(),
            operation,
        };
        let (state_tx, _) = watch::channel(state);
        let this = Self {
            inner: Arc::new(Inner {
                installer,
                updater,
                gate,
                idle,
                idle_poll: IDLE_POLL,
                record_path,
                state_tx,
                control: Mutex::new(None),
                shutdown: CancellationToken::new(),
                tasks: Mutex::new(Vec::new()),
                operation_gate: tokio::sync::Mutex::new(()),
            }),
        };
        this.follow_checker();
        this
    }

    #[cfg(test)]
    fn with_idle_poll(mut self, poll: Duration) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("configure before sharing")
            .idle_poll = poll;
        self
    }

    /// Judge the operation an earlier process left behind. Only the running
    /// version proves completion, and an outcome is kept only while it still
    /// describes this engine.
    fn recover(path: &Path, installer: &dyn EngineInstaller) -> Option<EngineUpdateOperation> {
        let record: OperationRecord = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        let mut operation = record.operation;
        let running = installer.running_version();
        let target = operation.target_version.clone().unwrap_or_default();
        let mut error = operation.error.clone();
        let phase = match operation.phase {
            // Never reached the install boundary: nothing changed, and the
            // release is still on offer.
            EngineUpdatePhase::Staging | EngineUpdatePhase::WaitingForIdle => None,
            // Settled outcomes stand as recorded, so a retry after another
            // restart joins the same request instead of installing twice.
            EngineUpdatePhase::Updated => (running == target).then_some(operation.phase),
            EngineUpdatePhase::Failed
            | EngineUpdatePhase::Cancelled
            | EngineUpdatePhase::Unknown => {
                (running == operation.from_version).then_some(operation.phase)
            }
            EngineUpdatePhase::Applying
            | EngineUpdatePhase::Restarting
            | EngineUpdatePhase::RestartRequired => {
                if running == target {
                    error = None;
                    Some(EngineUpdatePhase::Updated)
                } else if running != operation.from_version {
                    // Updated some other way since.
                    None
                } else if installer.installed_version().as_deref() == Some(target.as_str()) {
                    // Still pending: the restart that finishes it reads this.
                    Some(EngineUpdatePhase::RestartRequired)
                } else {
                    error = Some(if operation.phase == EngineUpdatePhase::Applying {
                        format!(
                            "the engine stopped while installing {target}; it still runs {running}"
                        )
                    } else {
                        format!("{target} is no longer installed; the engine still runs {running}")
                    });
                    Some(EngineUpdatePhase::Failed)
                }
            }
        };
        let Some(phase) = phase else {
            let _ = std::fs::remove_file(path);
            return None;
        };
        if phase != operation.phase {
            operation.phase = phase;
            operation.error = error;
            operation.updated_at = now_ms();
            let _ = write_record(path, &operation);
        }
        Some(operation)
    }

    /// Follow the release checker, and keep looking for a binary someone else
    /// installed even while the feed is quiet or unreachable.
    fn follow_checker(&self) {
        let Some(updater) = &self.inner.updater else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let mut statuses = updater.watch();
        let this = self.clone();
        let task = tokio::spawn(async move {
            let mut restarted_for = None;
            loop {
                let feed = tokio::select! {
                    _ = this.inner.shutdown.cancelled() => break,
                    changed = statuses.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        Some(statuses.borrow_and_update().clone())
                    }
                    _ = tokio::time::sleep(INSTALLED_POLL) => None,
                };
                let installed = this.inner.installer.installed_version();
                this.inner.state_tx.send_if_modified(|state| {
                    let mut next = state.clone();
                    next.installed_version = installed;
                    if let Some(status) = feed {
                        next.latest_version = status.latest_version;
                        next.checked_at = status.checked_at;
                        next.check_error = status.error;
                    }
                    let changed = next != *state;
                    *state = next;
                    changed
                });
                this.automatic(zeron_update::auto_update_enabled(), &mut restarted_for);
            }
        });
        lock(&self.inner.tasks).push(task);
    }

    /// What the engine starts on its own: the restart that finishes an update
    /// someone else installed, and with `install_new` the newest release.
    fn automatic(&self, install_new: bool, restarted_for: &mut Option<String>) {
        let state = self.state();
        let request = match state.installed_version.as_deref() {
            Some(installed) if state.restart_pending() => {
                // Once per installed version: a service manager that cannot
                // restart this engine must not become a restart loop that
                // keeps closing admission.
                if !self.inner.installer.supervised() || restarted_for.as_deref() == Some(installed)
                {
                    return;
                }
                *restarted_for = Some(installed.to_owned());
                format!("automatic:restart:{installed}")
            }
            // One attempt per successful check, so a failed download is
            // retried at the next one.
            _ if install_new
                && state.check_error.is_none()
                && state.update_available()
                && state.support.can_install() =>
            {
                format!(
                    "automatic:{}:{}",
                    state.latest_version.as_deref().unwrap_or_default(),
                    state.checked_at.unwrap_or_default()
                )
            }
            _ => return,
        };
        if let Err(error) = self.start(Some(request)) {
            tracing::debug!(%error, "automatic engine update not started");
        }
    }

    pub fn prepare_desktop_restart(&self) -> Result<DesktopRestartPermit, String> {
        let _control = lock(&self.inner.control);
        if self.inner.gate.closed() || !self.inner.gate.close_if(self.inner.idle.as_ref()) {
            return Err("Zeron is busy. Wait for agent runs and updates to finish, close open terminals, then restart to apply the desktop update.".into());
        }
        Ok(DesktopRestartPermit(Some(self.inner.gate.clone())))
    }

    pub fn watch(&self) -> watch::Receiver<EngineUpdateState> {
        self.inner.state_tx.subscribe()
    }

    pub fn state(&self) -> EngineUpdateState {
        self.inner.state_tx.borrow().clone()
    }

    /// Refresh release metadata. Never stages or installs.
    pub async fn check(&self) -> Result<EngineUpdateState, String> {
        let latest = match &self.inner.updater {
            Some(updater) => updater
                .check()
                .await
                .map(|status| (status.latest_version, status.checked_at))
                .map_err(|error| format!("{error:#}")),
            None => self
                .inner
                .installer
                .latest()
                .await
                .map(|version| (Some(version), Some(now_ms()))),
        };
        let installed = self.inner.installer.installed_version();
        self.inner.state_tx.send_modify(|state| {
            state.installed_version = installed;
            match &latest {
                Ok((version, checked_at)) => {
                    state.latest_version = version.clone();
                    state.checked_at = *checked_at;
                    state.check_error = None;
                }
                Err(error) => state.check_error = Some(error.clone()),
            }
        });
        latest.map(|_| self.state())
    }

    /// Accept an operation and return at once. A retry with the same
    /// `request_id`, or any request while one is in flight, returns the
    /// existing operation instead of starting another.
    pub fn start(&self, request_id: Option<String>) -> Result<EngineUpdateAck, String> {
        let mut control = lock(&self.inner.control);
        let state = self.state();
        if let Some(existing) = &state.operation
            && (!existing.phase.is_terminal()
                || (request_id.is_some() && existing.request_id == request_id))
        {
            return Ok(EngineUpdateAck {
                operation_id: existing.id.clone(),
                state,
            });
        }
        if self.inner.shutdown.is_cancelled() {
            return Err("the engine is shutting down".into());
        }
        let support = self.inner.installer.support();
        // A newer binary already on disk only needs the restart, whoever
        // installed it.
        let installed = self.inner.installer.installed_version();
        if let Some(installed) = installed
            .as_deref()
            .filter(|installed| zeron_update::version_newer(installed, &state.running_version))
        {
            if !self.inner.installer.supervised() {
                return Err(format!(
                    "{installed} is already installed; restart the engine to finish"
                ));
            }
        } else if !support.can_install() {
            return Err(match support {
                EngineUpdateSupport::Unsupported { reason } => reason,
                _ => "this engine can't install updates".into(),
            });
        }
        let now = now_ms();
        let operation = EngineUpdateOperation {
            id: uuid::Uuid::new_v4().to_string(),
            request_id,
            from_version: state.running_version.clone(),
            target_version: None,
            phase: EngineUpdatePhase::Staging,
            started_at: now,
            updated_at: now,
            error: None,
        };
        let cancel = self.inner.shutdown.child_token();
        *control = Some(cancel.clone());
        self.inner.state_tx.send_modify(|state| {
            state.support = support;
            state.installed_version = installed;
            state.operation = Some(operation.clone());
        });
        if let Err(error) = self.persist(&operation.id) {
            control.take();
            self.set_phase(
                &operation.id,
                EngineUpdatePhase::Failed,
                Some(error.clone()),
            );
            return Err(error);
        }
        drop(control);
        let this = self.clone();
        let id = operation.id.clone();
        let task = tokio::spawn(async move { this.run(id, cancel).await });
        lock(&self.inner.tasks).push(task);
        Ok(EngineUpdateAck {
            operation_id: operation.id,
            state: self.state(),
        })
    }

    /// Cancel before the apply boundary. The decision and the phase change
    /// happen under the same lock as the boundary crossing.
    pub fn cancel(&self, operation_id: &str) -> bool {
        let mut control = lock(&self.inner.control);
        let current = self.state().operation;
        let Some(operation) = current.filter(|op| op.id == operation_id) else {
            return false;
        };
        if !operation.phase.cancellable() {
            return false;
        }
        if let Some(cancel) = control.take() {
            cancel.cancel();
        }
        self.set_phase(operation_id, EngineUpdatePhase::Cancelled, None);
        let _ = self.persist(operation_id);
        true
    }

    fn set_phase(&self, id: &str, phase: EngineUpdatePhase, error: Option<String>) {
        self.inner.state_tx.send_modify(|state| {
            if let Some(operation) = state.operation.as_mut().filter(|op| op.id == id) {
                operation.phase = phase;
                operation.error = error;
                operation.updated_at = now_ms();
            }
        });
    }

    fn phase(&self, id: &str) -> Option<EngineUpdatePhase> {
        self.state()
            .operation
            .filter(|op| op.id == id)
            .map(|op| op.phase)
    }

    fn fail(&self, id: &str, error: String) {
        tracing::warn!(%error, "engine update failed");
        let mut control = lock(&self.inner.control);
        if self.phase(id).is_some_and(|phase| !phase.is_terminal()) {
            control.take();
            self.set_phase(id, EngineUpdatePhase::Failed, Some(error));
            let _ = self.persist(id);
        }
    }

    async fn run(self, id: String, cancel: CancellationToken) {
        let _operation = tokio::select! {
            _ = cancel.cancelled() => return,
            guard = self.inner.operation_gate.lock() => guard,
        };
        let identity_before = self.inner.installer.installation_identity();
        let installed_before = self.inner.installer.installed_version();
        let running = self.inner.installer.running_version();
        let restart_only = installed_before
            .as_ref()
            .filter(|v| zeron_update::version_newer(v, &running))
            .cloned();
        let latest = if let Some(installed) = &restart_only {
            Ok(installed.clone())
        } else {
            tokio::select! {
                _ = cancel.cancelled() => return,
                latest = self.inner.installer.latest() => latest,
            }
        };
        let target = match latest {
            Ok(version) => version,
            Err(error) => {
                return self.fail(&id, format!("could not read the release feed: {error}"));
            }
        };
        let running = self.inner.installer.running_version();
        if !zeron_update::version_newer(&target, &running) {
            return self.fail(&id, format!("{running} is already the newest release"));
        }
        self.inner.state_tx.send_modify(|state| {
            if restart_only.is_none() {
                state.latest_version = Some(target.clone());
            }
            if let Some(operation) = state.operation.as_mut().filter(|op| op.id == id) {
                operation.target_version = Some(target.clone());
                operation.updated_at = now_ms();
            }
        });
        let staged = if restart_only.is_some() {
            Ok(())
        } else {
            tokio::select! {
                _ = cancel.cancelled() => return,
                staged = self.inner.installer.stage(&target) => staged,
            }
        };
        if let Err(error) = staged {
            return self.fail(&id, format!("could not stage {target}: {error}"));
        }
        {
            let _control = lock(&self.inner.control);
            if cancel.is_cancelled() {
                return;
            }
            self.set_phase(&id, EngineUpdatePhase::WaitingForIdle, None);
        }
        // Close admission only at an idle instant, then cross the boundary
        // under the control lock so a concurrent cancel either wins before
        // Applying or is refused after it.
        loop {
            if cancel.is_cancelled() {
                return;
            }
            if self.inner.gate.close_if(self.inner.idle.as_ref()) {
                let control = lock(&self.inner.control);
                if cancel.is_cancelled() {
                    drop(control);
                    self.inner.gate.reopen();
                    return;
                }
                self.set_phase(&id, EngineUpdatePhase::Applying, None);
                break;
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(self.inner.idle_poll) => {}
            }
        }
        if self.inner.installer.installed_version() != installed_before
            || self.inner.installer.installation_identity() != identity_before
        {
            self.inner.gate.reopen();
            return self.fail(
                &id,
                "the installed version changed while preparing or waiting; check again".into(),
            );
        }
        self.apply(&id, &target, restart_only.is_some());
    }

    /// Past the boundary: not cancellable, admission closed.
    fn apply(&self, id: &str, target: &str, restart_only: bool) {
        if let Err(error) = self.persist(id) {
            self.inner.gate.reopen();
            return self.fail(id, error);
        }
        if let Err(error) = if restart_only {
            Ok(())
        } else {
            self.inner.installer.apply(target)
        } {
            self.inner.gate.reopen();
            return self.fail(id, format!("could not install {target}: {error}"));
        }
        let installed = self.inner.installer.installed_version();
        if installed.as_deref() != Some(target) {
            self.inner.gate.reopen();
            return self.fail(id, format!("installation verification failed: expected {target}, found {}. No restart was requested", installed.as_deref().unwrap_or("unknown")));
        }
        self.inner
            .state_tx
            .send_modify(|state| state.installed_version = installed);
        lock(&self.inner.control).take();
        if !self.inner.installer.supervised() {
            self.inner.gate.reopen();
            let reason = match self.inner.installer.support() {
                EngineUpdateSupport::ManualRestart { reason } => Some(reason),
                _ => None,
            };
            self.set_phase(id, EngineUpdatePhase::RestartRequired, reason);
            let _ = self.persist(id);
            return;
        }
        self.set_phase(id, EngineUpdatePhase::Restarting, None);
        if let Err(error) = self.persist(id) {
            self.inner.gate.reopen();
            self.set_phase(id, EngineUpdatePhase::RestartRequired, Some(error));
            return;
        }
        let this = self.clone();
        let id = id.to_owned();
        let task = tokio::spawn(async move {
            // Shutdown (often the supervisor's own SIGTERM) ends the
            // wait at once: graceful teardown must not sit behind it.
            // The on-disk record lets the next process judge the outcome.
            let shutdown = this.inner.shutdown.clone();
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = tokio::time::sleep(RESTART_GRACE) => {}
            }
            let error = match this.inner.installer.restart() {
                Ok(()) => {
                    tokio::select! {
                        _ = shutdown.cancelled() => return,
                        _ = tokio::time::sleep(RESTART_WATCHDOG) => {}
                    }
                    "the service manager accepted the restart but Zeron is still running"
                        .to_string()
                }
                Err(error) => {
                    format!("the service manager did not restart Zeron: {error}")
                }
            };
            // Installed but still serving the old binary. Keep
            // serving: work is admitted again until a restart.
            if this.phase(&id) == Some(EngineUpdatePhase::Restarting) {
                this.inner.gate.reopen();
                this.set_phase(&id, EngineUpdatePhase::RestartRequired, Some(error));
                let _ = this.persist(&id);
            }
        });
        lock(&self.inner.tasks).push(task);
    }

    fn persist(&self, id: &str) -> Result<(), String> {
        let operation = self
            .state()
            .operation
            .filter(|op| op.id == id)
            .ok_or_else(|| "engine update operation was replaced".to_string())?;
        write_record(&self.inner.record_path, &operation).map_err(|error| {
            format!("could not durably record engine update; installation/restart refused: {error}")
        })
    }

    /// Stop pre-boundary work. An operation past the boundary finishes on its
    /// own (the swap is synchronous and the restart is the supervisor's).
    pub async fn shutdown(&self) {
        self.inner.shutdown.cancel();
        let tasks: Vec<_> = lock(&self.inner.tasks).drain(..).collect();
        for task in tasks {
            let _ = task.await;
        }
    }
}

/// Durably replace the operation record.
fn write_record(path: &Path, operation: &EngineUpdateOperation) -> std::io::Result<()> {
    use std::io::Write;
    let json = serde_json::to_vec_pretty(&OperationRecord {
        operation: operation.clone(),
    })?;
    let temp = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&temp)?;
    file.write_all(&json)?;
    file.sync_all()?;
    std::fs::rename(&temp, path)?;
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeInstaller {
        running: String,
        installed: Mutex<Option<String>>,
        latest: String,
        support: Mutex<Option<EngineUpdateSupport>>,
        supervised: Mutex<Option<bool>>,
        stage_gate: tokio::sync::Notify,
        hold_staging: AtomicBool,
        fail_stage: AtomicBool,
        fail_apply: AtomicBool,
        fail_restart: AtomicBool,
        noop_apply: AtomicBool,
        stages: AtomicUsize,
        applies: AtomicUsize,
        restarts: AtomicUsize,
    }

    impl FakeInstaller {
        fn new(running: &str, latest: &str) -> Arc<Self> {
            Arc::new(Self {
                running: running.into(),
                installed: Mutex::new(Some(running.into())),
                latest: latest.into(),
                ..Default::default()
            })
        }
    }

    #[async_trait]
    impl EngineInstaller for FakeInstaller {
        fn support(&self) -> EngineUpdateSupport {
            lock(&self.support)
                .clone()
                .unwrap_or(EngineUpdateSupport::Managed)
        }
        fn supervised(&self) -> bool {
            lock(&self.supervised).unwrap_or(matches!(self.support(), EngineUpdateSupport::Managed))
        }
        fn running_version(&self) -> String {
            self.running.clone()
        }
        fn installed_version(&self) -> Option<String> {
            lock(&self.installed).clone()
        }
        async fn latest(&self) -> Result<String, String> {
            Ok(self.latest.clone())
        }
        async fn stage(&self, _version: &str) -> Result<(), String> {
            self.stages.fetch_add(1, Ordering::SeqCst);
            if self.hold_staging.load(Ordering::SeqCst) {
                self.stage_gate.notified().await;
            }
            if self.fail_stage.load(Ordering::SeqCst) {
                return Err("checksum mismatch".into());
            }
            Ok(())
        }
        fn apply(&self, version: &str) -> Result<(), String> {
            self.applies.fetch_add(1, Ordering::SeqCst);
            if self.fail_apply.load(Ordering::SeqCst) {
                return Err("atomic replacement refused".into());
            }
            if !self.noop_apply.load(Ordering::SeqCst) {
                *lock(&self.installed) = Some(version.into());
            }
            Ok(())
        }
        fn restart(&self) -> Result<(), String> {
            self.restarts.fetch_add(1, Ordering::SeqCst);
            if self.fail_restart.load(Ordering::SeqCst) {
                return Err("launchctl: service not found".into());
            }
            Ok(())
        }
    }

    struct Fixture {
        temp: tempfile::TempDir,
        installer: Arc<FakeInstaller>,
        busy: Arc<AtomicBool>,
        gate: RestartGate,
        updates: EngineUpdates,
    }

    fn fixture(installer: Arc<FakeInstaller>) -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let busy = Arc::new(AtomicBool::new(false));
        let gate = RestartGate::default();
        let idle = {
            let busy = busy.clone();
            Arc::new(move || !busy.load(Ordering::SeqCst))
        };
        let updates = EngineUpdates::new(temp.path(), installer.clone(), None, gate.clone(), idle)
            .with_idle_poll(Duration::from_millis(10));
        Fixture {
            temp,
            installer,
            busy,
            gate,
            updates,
        }
    }

    async fn wait_for(updates: &EngineUpdates, phase: EngineUpdatePhase) -> EngineUpdateOperation {
        let mut watch = updates.watch();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(op) = updates.state().operation.filter(|op| op.phase == phase) {
                    return op;
                }
                watch.changed().await.unwrap();
            }
        })
        .await
        .unwrap_or_else(|_| panic!("expected {phase:?}, got {:?}", updates.state().operation))
    }

    #[tokio::test]
    async fn desktop_reservation_rejects_busy_and_reopens_after_failure() {
        let f = fixture(FakeInstaller::new("0.2.90", "0.2.97"));
        f.busy.store(true, Ordering::SeqCst);
        assert!(f.updates.prepare_desktop_restart().is_err());
        f.busy.store(false, Ordering::SeqCst);
        let permit = f.updates.prepare_desktop_restart().unwrap();
        assert!(f.gate.admit().await.is_err());
        assert!(f.updates.prepare_desktop_restart().is_err());
        drop(permit);
        assert!(f.gate.admit().await.is_ok());
        f.updates.prepare_desktop_restart().unwrap().commit();
        assert!(f.gate.try_admit().is_err());
    }

    #[tokio::test]
    async fn zero_exit_without_installing_never_restarts_or_claims_success() {
        let f = fixture(FakeInstaller::new("0.2.90", "0.2.97"));
        f.installer.noop_apply.store(true, Ordering::SeqCst);
        f.updates.start(None).unwrap();
        let op = wait_for(&f.updates, EngineUpdatePhase::Failed).await;
        assert!(op.error.unwrap().contains("verification failed"));
        assert_eq!(f.installer.restarts.load(Ordering::SeqCst), 0);
        assert!(f.gate.try_admit().is_ok());
    }

    #[tokio::test]
    async fn changed_installation_while_waiting_does_not_downgrade() {
        let f = fixture(FakeInstaller::new("0.2.90", "0.2.97"));
        f.busy.store(true, Ordering::SeqCst);
        f.updates.start(None).unwrap();
        wait_for(&f.updates, EngineUpdatePhase::WaitingForIdle).await;
        *lock(&f.installer.installed) = Some("0.2.99".into());
        f.busy.store(false, Ordering::SeqCst);
        wait_for(&f.updates, EngineUpdatePhase::Failed).await;
        assert_eq!(f.installer.applies.load(Ordering::SeqCst), 0);
        assert!(f.gate.try_admit().is_ok());
    }

    #[tokio::test]
    async fn unwritable_operation_record_refuses_mutation() {
        let f = fixture(FakeInstaller::new("0.2.90", "0.2.97"));
        std::fs::create_dir(f.temp.path().join(RECORD_FILE).with_extension("json.tmp")).unwrap();
        assert!(
            f.updates
                .start(None)
                .unwrap_err()
                .contains("durably record")
        );
        assert_eq!(f.installer.applies.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn completed_request_survives_two_restarts() {
        let f = fixture(FakeInstaller::new("0.2.90", "0.2.97"));
        let ack = f.updates.start(Some("durable-request".into())).unwrap();
        wait_for(&f.updates, EngineUpdatePhase::Restarting).await;
        f.updates.shutdown().await;
        for _ in 0..2 {
            let installer = FakeInstaller::new("0.2.97", "0.2.99");
            let after = EngineUpdates::new(
                f.temp.path(),
                installer.clone(),
                None,
                RestartGate::default(),
                Arc::new(|| true),
            );
            let retry = after.start(Some("durable-request".into())).unwrap();
            assert_eq!(retry.operation_id, ack.operation_id);
            assert_eq!(
                retry.state.operation.unwrap().phase,
                EngineUpdatePhase::Updated
            );
            assert_eq!(installer.applies.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn waits_for_idle_then_installs_restarts_and_records_the_target() {
        let f = fixture(FakeInstaller::new("0.2.90", "0.2.97"));
        f.busy.store(true, Ordering::SeqCst);
        let ack = f.updates.start(Some("req-1".into())).unwrap();
        let waiting = wait_for(&f.updates, EngineUpdatePhase::WaitingForIdle).await;
        assert_eq!(waiting.id, ack.operation_id);
        assert_eq!(waiting.target_version.as_deref(), Some("0.2.97"));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(f.installer.applies.load(Ordering::SeqCst), 0);
        assert!(
            f.gate.try_admit().is_ok(),
            "busy engines keep admitting work"
        );

        f.busy.store(false, Ordering::SeqCst);
        wait_for(&f.updates, EngineUpdatePhase::Restarting).await;
        assert_eq!(f.installer.applies.load(Ordering::SeqCst), 1);
        assert!(
            f.gate.admit().await.is_err(),
            "no new run may start under the restart"
        );
        tokio::time::sleep(RESTART_GRACE + Duration::from_millis(200)).await;
        assert_eq!(f.installer.restarts.load(Ordering::SeqCst), 1);
        assert!(f.temp.path().join(RECORD_FILE).is_file());
    }

    #[tokio::test]
    async fn the_restarted_engine_reports_updated_only_on_the_target_version() {
        let f = fixture(FakeInstaller::new("0.2.90", "0.2.97"));
        f.updates.start(None).unwrap();
        wait_for(&f.updates, EngineUpdatePhase::Restarting).await;
        let id = f.updates.state().operation.unwrap().id;

        // The supervisor brought back the new binary.
        let restarted = FakeInstaller::new("0.2.97", "0.2.97");
        let after = EngineUpdates::new(
            f.temp.path(),
            restarted,
            None,
            RestartGate::default(),
            Arc::new(|| true),
        );
        let op = after.state().operation.unwrap();
        assert_eq!(
            (op.id.as_str(), op.phase),
            (id.as_str(), EngineUpdatePhase::Updated)
        );
        assert!(f.temp.path().join(RECORD_FILE).exists());
    }

    #[tokio::test]
    async fn installed_but_still_running_the_old_version_is_restart_required() {
        let f = fixture(FakeInstaller::new("0.2.90", "0.2.97"));
        f.updates.start(None).unwrap();
        wait_for(&f.updates, EngineUpdatePhase::Restarting).await;
        // Something restarted the old binary instead (for example by hand).
        let stale = FakeInstaller::new("0.2.90", "0.2.97");
        *lock(&stale.installed) = Some("0.2.97".into());
        let after = EngineUpdates::new(
            f.temp.path(),
            stale.clone(),
            None,
            RestartGate::default(),
            Arc::new(|| true),
        );
        let state = after.state();
        assert_eq!(
            state.operation.as_ref().unwrap().phase,
            EngineUpdatePhase::RestartRequired
        );
        assert!(state.restart_pending());
        after.start(None).unwrap();
        wait_for(&after, EngineUpdatePhase::Restarting).await;
        assert_eq!(stale.stages.load(Ordering::SeqCst), 0);
        assert_eq!(stale.applies.load(Ordering::SeqCst), 0);
        after.shutdown().await;
    }

    #[tokio::test]
    async fn a_failed_supervisor_restart_reopens_admission_and_asks_for_a_restart() {
        let installer = FakeInstaller::new("0.2.90", "0.2.97");
        installer.fail_restart.store(true, Ordering::SeqCst);
        let f = fixture(installer);
        f.updates.start(None).unwrap();
        let op = wait_for(&f.updates, EngineUpdatePhase::RestartRequired).await;
        assert!(op.error.unwrap().contains("service not found"));
        assert!(f.gate.admit().await.is_ok());
    }

    #[tokio::test]
    async fn an_unsupervised_engine_installs_but_never_restarts_itself() {
        let installer = FakeInstaller::new("0.2.90", "0.2.97");
        *lock(&installer.support) = Some(EngineUpdateSupport::ManualRestart {
            reason: "started by hand".into(),
        });
        let f = fixture(installer);
        f.updates.start(None).unwrap();
        wait_for(&f.updates, EngineUpdatePhase::RestartRequired).await;
        tokio::time::sleep(RESTART_GRACE + Duration::from_millis(100)).await;
        assert_eq!(f.installer.restarts.load(Ordering::SeqCst), 0);
        assert!(f.gate.try_admit().is_ok());
    }

    #[tokio::test]
    async fn retries_and_second_clients_join_the_operation_in_flight() {
        let installer = FakeInstaller::new("0.2.90", "0.2.97");
        installer.hold_staging.store(true, Ordering::SeqCst);
        let f = fixture(installer);
        let first = f.updates.start(Some("a".into())).unwrap();
        let retry = f.updates.start(Some("a".into())).unwrap();
        let other_client = f.updates.start(Some("b".into())).unwrap();
        assert_eq!(first.operation_id, retry.operation_id);
        assert_eq!(first.operation_id, other_client.operation_id);
        f.installer.stage_gate.notify_one();
        wait_for(&f.updates, EngineUpdatePhase::Restarting).await;
        // A response lost after completion: the same key reconciles, it does
        // not replay the install.
        let again = f.updates.start(Some("a".into())).unwrap();
        assert_eq!(again.operation_id, first.operation_id);
        assert_eq!(f.installer.stages.load(Ordering::SeqCst), 1);
        assert_eq!(f.installer.applies.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancellation_before_the_boundary_prevents_installation() {
        let f = fixture(FakeInstaller::new("0.2.90", "0.2.97"));
        f.busy.store(true, Ordering::SeqCst);
        let ack = f.updates.start(None).unwrap();
        wait_for(&f.updates, EngineUpdatePhase::WaitingForIdle).await;
        assert!(f.updates.cancel(&ack.operation_id));
        f.busy.store(false, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(f.installer.applies.load(Ordering::SeqCst), 0);
        assert_eq!(
            f.updates.state().operation.unwrap().phase,
            EngineUpdatePhase::Cancelled
        );
        assert!(f.gate.try_admit().is_ok());
        assert!(!f.updates.cancel("someone-else"));
    }

    #[tokio::test]
    async fn cancellation_after_the_boundary_is_refused() {
        let f = fixture(FakeInstaller::new("0.2.90", "0.2.97"));
        let ack = f.updates.start(None).unwrap();
        wait_for(&f.updates, EngineUpdatePhase::Restarting).await;
        assert!(!f.updates.cancel(&ack.operation_id));
        assert_eq!(
            f.updates.state().operation.unwrap().phase,
            EngineUpdatePhase::Restarting
        );
    }

    #[tokio::test]
    async fn work_admitted_before_idle_is_observed_blocks_the_restart() {
        let f = fixture(FakeInstaller::new("0.2.90", "0.2.97"));
        // A run holds its ticket while it registers; the gate must not close
        // beneath it even though `busy` has not flipped yet.
        let ticket = f.gate.admit().await.unwrap();
        f.updates.start(None).unwrap();
        wait_for(&f.updates, EngineUpdatePhase::WaitingForIdle).await;
        f.busy.store(true, Ordering::SeqCst);
        drop(ticket);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(f.installer.applies.load(Ordering::SeqCst), 0);
        f.busy.store(false, Ordering::SeqCst);
        wait_for(&f.updates, EngineUpdatePhase::Restarting).await;
    }

    #[tokio::test]
    async fn corrupt_staging_fails_without_touching_the_installation() {
        let installer = FakeInstaller::new("0.2.90", "0.2.97");
        installer.fail_stage.store(true, Ordering::SeqCst);
        let f = fixture(installer);
        f.updates.start(None).unwrap();
        let op = wait_for(&f.updates, EngineUpdatePhase::Failed).await;
        assert!(op.error.unwrap().contains("checksum mismatch"));
        assert_eq!(f.installer.applies.load(Ordering::SeqCst), 0);
        assert!(f.gate.try_admit().is_ok());
        // A failed operation does not block a fresh request.
        f.installer.fail_stage.store(false, Ordering::SeqCst);
        let retry = f.updates.start(None).unwrap();
        assert_ne!(retry.operation_id, op.id);
    }

    #[tokio::test]
    async fn failed_apply_survives_restart_and_deduplicates_retry() {
        let installer = FakeInstaller::new("0.2.90", "0.2.97");
        installer.fail_apply.store(true, Ordering::SeqCst);
        let f = fixture(installer);
        let first = f.updates.start(Some("failed-apply".into())).unwrap();
        let failed = wait_for(&f.updates, EngineUpdatePhase::Failed).await;
        assert!(failed.error.unwrap().contains("atomic replacement refused"));
        assert!(f.gate.try_admit().is_ok());
        f.updates.shutdown().await;

        let restarted = FakeInstaller::new("0.2.90", "0.2.97");
        let after = EngineUpdates::new(
            f.temp.path(),
            restarted.clone(),
            None,
            RestartGate::default(),
            Arc::new(|| true),
        );
        let retry = after.start(Some("failed-apply".into())).unwrap();
        assert_eq!(retry.operation_id, first.operation_id);
        assert_eq!(
            retry.state.operation.unwrap().phase,
            EngineUpdatePhase::Failed
        );
        assert_eq!(restarted.applies.load(Ordering::SeqCst), 0);
        assert_eq!(restarted.stages.load(Ordering::SeqCst), 0);
    }

    /// A new process over the same data directory, as after a restart.
    fn reboot(f: &Fixture, installer: Arc<FakeInstaller>) -> EngineUpdates {
        EngineUpdates::new(
            f.temp.path(),
            installer,
            None,
            RestartGate::default(),
            Arc::new(|| true),
        )
    }

    #[tokio::test]
    async fn an_update_interrupted_before_installing_is_forgotten_not_failed() {
        let f = fixture(FakeInstaller::new("0.2.90", "0.2.97"));
        f.busy.store(true, Ordering::SeqCst);
        f.updates.start(Some("when-idle".into())).unwrap();
        wait_for(&f.updates, EngineUpdatePhase::WaitingForIdle).await;
        // The device reboots while the update still waits for idle.
        f.updates.shutdown().await;
        let after = reboot(&f, FakeInstaller::new("0.2.90", "0.2.97"));
        assert!(after.state().operation.is_none());
        assert!(!f.temp.path().join(RECORD_FILE).exists());
    }

    #[tokio::test]
    async fn settled_outcomes_are_not_judged_again_and_expire_with_their_version() {
        let f = fixture(FakeInstaller::new("0.2.90", "0.2.97"));
        f.updates.start(None).unwrap();
        wait_for(&f.updates, EngineUpdatePhase::Restarting).await;
        f.updates.shutdown().await;
        let updated = reboot(&f, FakeInstaller::new("0.2.97", "0.2.97"))
            .state()
            .operation
            .unwrap();
        assert_eq!(updated.phase, EngineUpdatePhase::Updated);
        tokio::time::sleep(Duration::from_millis(5)).await;
        // A later restart must not announce the same update again.
        let again = reboot(&f, FakeInstaller::new("0.2.97", "0.2.97"));
        assert_eq!(again.state().operation, Some(updated));
        // `zeron update` on the device moved past it: nothing left to say.
        let later = reboot(&f, FakeInstaller::new("0.2.99", "0.2.99"));
        assert!(later.state().operation.is_none());
        assert!(!f.temp.path().join(RECORD_FILE).exists());
    }

    #[tokio::test]
    async fn a_failure_fixed_outside_zeron_does_not_stay_on_the_engine() {
        let installer = FakeInstaller::new("0.2.90", "0.2.97");
        installer.fail_apply.store(true, Ordering::SeqCst);
        let f = fixture(installer);
        f.updates.start(None).unwrap();
        wait_for(&f.updates, EngineUpdatePhase::Failed).await;
        f.updates.shutdown().await;
        let after = reboot(&f, FakeInstaller::new("0.2.97", "0.2.97"));
        assert!(after.state().operation.is_none());
    }

    #[tokio::test]
    async fn a_supervised_engine_restarts_into_a_binary_it_cannot_install_itself() {
        // A launchd daemon inside the app bundle the desktop app just swapped.
        let installer = FakeInstaller::new("0.2.90", "0.2.97");
        *lock(&installer.support) = Some(EngineUpdateSupport::Unsupported {
            reason: "part of the desktop app".into(),
        });
        *lock(&installer.supervised) = Some(true);
        *lock(&installer.installed) = Some("0.2.97".into());
        let f = fixture(installer);
        let mut restarted_for = None;
        f.updates.automatic(false, &mut restarted_for);
        let op = wait_for(&f.updates, EngineUpdatePhase::Restarting).await;
        assert_eq!(op.target_version.as_deref(), Some("0.2.97"));
        assert_eq!(f.installer.stages.load(Ordering::SeqCst), 0);
        assert_eq!(f.installer.applies.load(Ordering::SeqCst), 0);
        assert!(f.gate.try_admit().is_err());
    }

    #[tokio::test]
    async fn a_restart_the_service_manager_refuses_is_not_retried_for_that_version() {
        let installer = FakeInstaller::new("0.2.90", "0.2.97");
        installer.fail_restart.store(true, Ordering::SeqCst);
        *lock(&installer.installed) = Some("0.2.97".into());
        let f = fixture(installer);
        let mut restarted_for = None;
        f.updates.automatic(false, &mut restarted_for);
        let first = wait_for(&f.updates, EngineUpdatePhase::RestartRequired).await;
        for _ in 0..3 {
            f.updates.automatic(true, &mut restarted_for);
        }
        assert_eq!(f.updates.state().operation.unwrap().id, first.id);
        assert_eq!(f.installer.restarts.load(Ordering::SeqCst), 1);
        assert!(f.gate.try_admit().is_ok());
        // A newer installed binary is a new reason to restart.
        *lock(&f.installer.installed) = Some("0.2.98".into());
        f.updates
            .inner
            .state_tx
            .send_modify(|state| state.installed_version = Some("0.2.98".into()));
        f.updates.automatic(false, &mut restarted_for);
        assert_ne!(f.updates.state().operation.unwrap().id, first.id);
    }

    #[tokio::test]
    async fn automatic_installs_try_once_per_check_and_only_when_enabled() {
        let installer = FakeInstaller::new("0.2.90", "0.2.97");
        installer.fail_stage.store(true, Ordering::SeqCst);
        let f = fixture(installer);
        f.updates.check().await.unwrap();
        let mut restarted_for = None;
        f.updates.automatic(false, &mut restarted_for);
        assert!(f.updates.state().operation.is_none());
        f.updates.automatic(true, &mut restarted_for);
        let failed = wait_for(&f.updates, EngineUpdatePhase::Failed).await;
        f.updates.automatic(true, &mut restarted_for);
        assert_eq!(f.updates.state().operation.unwrap().id, failed.id);
        assert_eq!(f.installer.stages.load(Ordering::SeqCst), 1);
        // The next successful check is a new attempt.
        tokio::time::sleep(Duration::from_millis(5)).await;
        f.updates.check().await.unwrap();
        f.updates.automatic(true, &mut restarted_for);
        assert_ne!(f.updates.state().operation.unwrap().id, failed.id);
    }

    #[tokio::test]
    async fn unsupported_installs_explain_instead_of_starting() {
        let installer = FakeInstaller::new("0.2.90", "0.2.97");
        *lock(&installer.support) = Some(EngineUpdateSupport::Unsupported {
            reason: "part of the desktop app".into(),
        });
        let f = fixture(installer);
        assert_eq!(
            f.updates.start(None).unwrap_err(),
            "part of the desktop app"
        );
        assert!(f.updates.state().operation.is_none());
    }

    #[tokio::test]
    async fn shutdown_during_the_restart_watchdog_returns_promptly() {
        let f = fixture(FakeInstaller::new("0.2.90", "0.2.97"));
        f.updates.start(None).unwrap();
        wait_for(&f.updates, EngineUpdatePhase::Restarting).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while f.installer.restarts.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        // The supervisor's SIGTERM arrives while the watchdog waits.
        tokio::time::timeout(Duration::from_secs(2), f.updates.shutdown())
            .await
            .expect("shutdown waited out the restart watchdog");
        assert!(f.temp.path().join(RECORD_FILE).is_file());
    }

    #[tokio::test]
    async fn shutdown_before_the_boundary_installs_nothing() {
        let installer = FakeInstaller::new("0.2.90", "0.2.97");
        installer.hold_staging.store(true, Ordering::SeqCst);
        let f = fixture(installer);
        f.updates.start(None).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        tokio::time::timeout(Duration::from_secs(2), f.updates.shutdown())
            .await
            .unwrap();
        assert_eq!(f.installer.applies.load(Ordering::SeqCst), 0);
    }
}
