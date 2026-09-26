//! Device-local monitoring and safe mutation of independently-installed agent
//! CLIs. This deliberately does no work from `ListHarnesses`: all subprocess
//! and network probes live behind this coordinator and its watch stream.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use zeron_harness::process::{Command, Stdio};

use zeron_proto::{
    HarnessId, HarnessInstallSource, HarnessUpdateFailure, HarnessUpdatePhase, HarnessUpdatePolicy,
    HarnessUpdateProgress, HarnessUpdateStatus,
};

use crate::now_ms;
use crate::registry::HarnessRegistry;

const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const MAX_JITTER: u64 = 30 * 60;
const FIRST_RETRY: Duration = Duration::from_secs(5 * 60);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const UPDATE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Preferences {
    policies: HashMap<HarnessId, HarnessUpdatePolicy>,
    dismissed_versions: HashMap<HarnessId, String>,
}

#[derive(Clone, Copy)]
enum LatestSource {
    Claude,
    Npm(&'static str),
    Opencode,
    Github {
        repository: &'static str,
        tag_prefix: &'static str,
    },
    Command(&'static [&'static str]),
    Hermes,
    Manual,
}

enum UpdateCheck {
    Version(String),
    Available,
    Current,
    Manual,
}

struct ProviderSpec {
    version_args: &'static [&'static str],
    latest: LatestSource,
    update_args: Option<&'static [&'static str]>,
    manual_command: &'static str,
}

#[derive(Debug, Clone)]
struct CodexStandaloneInstall {
    root: PathBuf,
    target: String,
}

enum UpdatePlan {
    Command {
        executable: PathBuf,
        args: &'static [&'static str],
    },
    CodexStandalone(CodexStandaloneInstall),
}

struct ReleaseAsset {
    url: String,
    digest: String,
    size: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexPackageManifest {
    layout_version: u32,
    version: String,
    target: String,
    variant: String,
    entrypoint: String,
    resources_dir: String,
    path_dir: String,
}

/// Held for the complete standalone install so Zeron cannot race the Codex
/// installer (which uses the same lock file) or another Zeron process.
struct InstallFileLock {
    file: File,
}

impl InstallFileLock {
    fn acquire(root: &Path) -> Result<Self, String> {
        let path = root.join("install.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("could not open Codex install lock: {error}"))?;

        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd as _;
            loop {
                let result =
                    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                if result == 0 {
                    break;
                }
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
                    return Err("another Codex installation is already in progress".into());
                }
                return Err(format!("could not lock the Codex installation: {error}"));
            }
        }

        Ok(Self { file })
    }
}

impl Drop for InstallFileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd as _;
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

/// Removes only nonce-scoped artifacts made by one update attempt. Installed
/// releases remain immutable and are deliberately never deleted here.
struct CodexStageCleanup {
    archive: PathBuf,
    staging: Option<PathBuf>,
}

impl CodexStageCleanup {
    fn new(archive: PathBuf, staging: PathBuf) -> Self {
        Self {
            archive,
            staging: Some(staging),
        }
    }

    fn release_staging(&mut self) {
        self.staging = None;
    }
}

impl Drop for CodexStageCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.archive);
        if let Some(staging) = self.staging.take() {
            let _ = std::fs::remove_dir_all(staging);
        }
    }
}

fn provider(id: HarnessId) -> ProviderSpec {
    match id {
        HarnessId::ClaudeCode => ProviderSpec {
            version_args: &["--version"],
            latest: LatestSource::Claude,
            update_args: Some(&["update"]),
            manual_command: "claude update",
        },
        HarnessId::Codex => ProviderSpec {
            version_args: &["--version"],
            latest: LatestSource::Github {
                repository: "openai/codex",
                tag_prefix: "rust-v",
            },
            // Codex has no self-update command. npm/Homebrew remain manual;
            // `update_plan` separately recognizes the official standalone
            // release layout, where an atomic in-place update is unambiguous.
            update_args: None,
            manual_command: "Update Codex with its original installer or package manager",
        },
        HarnessId::Cursor => ProviderSpec {
            version_args: &["--version"],
            latest: LatestSource::Manual,
            update_args: Some(&["update"]),
            manual_command: "cursor-agent update",
        },
        HarnessId::Grok => ProviderSpec {
            version_args: &["version"],
            latest: LatestSource::Command(&["update", "--check"]),
            update_args: Some(&["update"]),
            manual_command: "grok update",
        },
        HarnessId::Hermes => ProviderSpec {
            version_args: &["--version"],
            latest: LatestSource::Hermes,
            update_args: Some(&["update", "--yes"]),
            manual_command: "hermes update",
        },
        HarnessId::Pi => ProviderSpec {
            version_args: &["--version"],
            latest: LatestSource::Npm("@earendil-works/pi-coding-agent"),
            update_args: Some(&["update", "--self"]),
            manual_command: "pi update --self",
        },
        HarnessId::Opencode => ProviderSpec {
            version_args: &["--version"],
            latest: LatestSource::Opencode,
            update_args: Some(&["upgrade"]),
            manual_command: "opencode upgrade",
        },
        HarnessId::Devin => ProviderSpec {
            version_args: &["--version"],
            latest: LatestSource::Manual,
            update_args: None,
            manual_command: "devin --version",
        },
        HarnessId::Antigravity => ProviderSpec {
            version_args: &["--version"],
            latest: LatestSource::Manual,
            // Zeron installs a pinned ACP server archive. It has no registered
            // self-update command; custom binaries remain installer-managed.
            update_args: None,
            manual_command: "Update Zeron or replace the configured Antigravity ACP server",
        },
        HarnessId::Mock => ProviderSpec {
            version_args: &["--version"],
            latest: LatestSource::Manual,
            update_args: None,
            manual_command: "",
        },
    }
}

fn update_plan(harness: HarnessId, executable: &Path) -> Result<UpdatePlan, String> {
    if harness == HarnessId::ClaudeCode && claude_package_manager_command(executable).is_some() {
        return Err("update Claude Code with its package manager".into());
    }
    if let Some(args) = provider(harness).update_args {
        return Ok(UpdatePlan::Command {
            executable: executable.to_path_buf(),
            args,
        });
    }
    if harness == HarnessId::Codex
        && let Some(install) = codex_standalone_install(executable)
    {
        return Ok(UpdatePlan::CodexStandalone(install));
    }
    Err("this provider requires a manual update".into())
}

fn can_apply_update(harness: HarnessId, executable: &Path) -> bool {
    update_plan(harness, executable).is_ok()
}

struct ActiveUpdate {
    cancel: CancellationToken,
    automatic: bool,
    previous_phase: HarnessUpdatePhase,
}

struct Inner {
    registry: Arc<HarnessRegistry>,
    order: Vec<HarnessId>,
    prefs_path: PathBuf,
    prefs: Mutex<Preferences>,
    statuses: Mutex<HashMap<HarnessId, HarnessUpdateStatus>>,
    status_tx: watch::Sender<Vec<HarnessUpdateStatus>>,
    cancellations: Mutex<HashMap<HarnessId, ActiveUpdate>>,
    operation_gates: Mutex<HashMap<HarnessId, Arc<tokio::sync::Mutex<()>>>>,
    check_slots: tokio::sync::Semaphore,
    shutdown: CancellationToken,
    worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
    client: reqwest::Client,
}

/// Cloneable engine service exposed to RPC and the periodic worker.
#[derive(Clone)]
pub struct HarnessUpdateCoordinator {
    inner: Arc<Inner>,
}

/// Cancellation-safety for an RPC/app task disappearing mid-update. Dropping
/// the future must never leave the registry's pending marker set forever.
struct UpdateIntentGuard {
    coordinator: HarnessUpdateCoordinator,
    harness: HarnessId,
    complete: bool,
}

impl UpdateIntentGuard {
    fn finish(&mut self) {
        self.complete = true;
    }
}

impl Drop for UpdateIntentGuard {
    fn drop(&mut self) {
        if self.complete {
            return;
        }
        let previous_phase = lock(&self.coordinator.inner.cancellations)
            .remove(&self.harness)
            .map(|update| update.previous_phase)
            .unwrap_or(HarnessUpdatePhase::ManualActionRequired);
        self.coordinator.inner.registry.end_update(self.harness);
        self.coordinator.mutate(self.harness, |status| {
            if matches!(
                status.phase,
                HarnessUpdatePhase::Installing | HarnessUpdatePhase::Verifying
            ) {
                status.phase = HarnessUpdatePhase::Failed;
                status.error = Some(HarnessUpdateFailure {
                    message: "update ended before verification; inspect the CLI installation"
                        .into(),
                    retryable: true,
                });
            } else {
                status.phase = if status.policy == HarnessUpdatePolicy::Off {
                    HarnessUpdatePhase::Dormant
                } else {
                    previous_phase
                };
            }
        });
    }
}

impl HarnessUpdateCoordinator {
    pub fn new(data_dir: &Path, registry: Arc<HarnessRegistry>) -> Self {
        let prefs_path = data_dir.join("harness-update-prefs.json");
        let prefs: Preferences = std::fs::read_to_string(&prefs_path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let order: Vec<_> = registry
            .descriptors()
            .into_iter()
            .filter(|descriptor| descriptor.id != HarnessId::Mock)
            .map(|descriptor| descriptor.id)
            .collect();
        let enabled = registry.enabled_set();
        let statuses: HashMap<_, _> = order
            .iter()
            .copied()
            .map(|harness| {
                let policy = prefs.policies.get(&harness).copied().unwrap_or_default();
                let phase = if enabled.contains(&harness) && policy != HarnessUpdatePolicy::Off {
                    HarnessUpdatePhase::Checking
                } else {
                    HarnessUpdatePhase::Dormant
                };
                (
                    harness,
                    HarnessUpdateStatus {
                        harness,
                        installed_version: None,
                        latest_version: None,
                        channel: Some("stable".into()),
                        source: HarnessInstallSource::Unknown,
                        policy,
                        phase,
                        progress: None,
                        checked_at: None,
                        error: None,
                        can_apply: provider(harness).update_args.is_some(),
                        manual_command: Some(provider(harness).manual_command.to_string())
                            .filter(|command| !command.is_empty()),
                    },
                )
            })
            .collect();
        let initial = ordered_snapshot(&order, &statuses);
        let (status_tx, _) = watch::channel(initial);
        Self {
            inner: Arc::new(Inner {
                registry,
                order,
                prefs_path,
                prefs: Mutex::new(prefs),
                statuses: Mutex::new(statuses),
                status_tx,
                cancellations: Mutex::new(HashMap::new()),
                operation_gates: Mutex::new(HashMap::new()),
                check_slots: tokio::sync::Semaphore::new(2),
                shutdown: CancellationToken::new(),
                worker: Mutex::new(None),
                client: reqwest::Client::builder()
                    .user_agent(concat!("zeron/", env!("CARGO_PKG_VERSION")))
                    .timeout(COMMAND_TIMEOUT)
                    .build()
                    .unwrap_or_default(),
            }),
        }
    }

    /// Start the immediate check plus the six-hour jittered/retry loop. Bare
    /// synchronous test assemblies simply omit the worker and can still use
    /// the explicit RPC methods once running under Tokio.
    pub fn start(&self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let mut worker = lock(&self.inner.worker);
        if worker.is_some() {
            return;
        }
        let coordinator = self.clone();
        *worker = Some(tokio::spawn(async move {
            let mut retry = FIRST_RETRY;
            loop {
                // Shutdown must not wait out slow probes or registry requests.
                let snapshot = tokio::select! {
                    _ = coordinator.inner.shutdown.cancelled() => break,
                    snapshot = coordinator.check_all() => snapshot,
                };
                let failed = snapshot
                    .iter()
                    .any(|status| status.phase == HarnessUpdatePhase::Failed);
                let delay = if failed {
                    let current = retry;
                    retry = (retry * 2).min(CHECK_INTERVAL);
                    current
                } else {
                    retry = FIRST_RETRY;
                    let jitter = (now_ms().unsigned_abs() % MAX_JITTER) + 1;
                    CHECK_INTERVAL + Duration::from_secs(jitter)
                };
                tokio::select! {
                    _ = coordinator.inner.shutdown.cancelled() => break,
                    _ = tokio::time::sleep(delay) => {}
                }
            }
        }));
    }

    pub fn watch(&self) -> watch::Receiver<Vec<HarnessUpdateStatus>> {
        self.inner.status_tx.subscribe()
    }

    pub fn snapshot(&self) -> Vec<HarnessUpdateStatus> {
        ordered_snapshot(&self.inner.order, &lock(&self.inner.statuses))
    }

    pub async fn check_all(&self) -> Vec<HarnessUpdateStatus> {
        let enabled = self.inner.registry.enabled_set();
        // Disabled rows remain visible in Settings but never spawn a probe.
        for id in &self.inner.order {
            let policy = self.policy(*id);
            if (!enabled.contains(id) || policy == HarnessUpdatePolicy::Off)
                && !self.is_mutating(*id)
            {
                self.mutate(*id, |status| {
                    status.phase = HarnessUpdatePhase::Dormant;
                    status.progress = None;
                    status.error = None;
                });
            }
        }
        futures::stream::iter(enabled)
            .filter(|id| futures::future::ready(self.policy(*id) != HarnessUpdatePolicy::Off))
            .map(|id| {
                let coordinator = self.clone();
                async move { coordinator.check_one(id).await }
            })
            .buffer_unordered(2)
            .collect::<Vec<_>>()
            .await;

        self.snapshot()
    }

    pub async fn check_one(&self, harness: HarnessId) -> Result<(), String> {
        self.check_one_inner(harness).await?;
        // Every successful discovery, including policy changes and single-row
        // retries, gets the same automatic-install behavior. The check's
        // operation lock has been released before scheduling the mutation.
        self.schedule_automatic_update(harness);
        Ok(())
    }

    fn automatic_update_ready(&self, harness: HarnessId) -> bool {
        let status = self.status(harness);
        !self.inner.shutdown.is_cancelled()
            && self.inner.registry.enabled_set().contains(&harness)
            && status.policy == HarnessUpdatePolicy::AutoWhenIdle
            && status.phase == HarnessUpdatePhase::Available
            && status.can_apply
    }

    fn schedule_automatic_update(&self, harness: HarnessId) {
        if !self.automatic_update_ready(harness) {
            return;
        }
        let coordinator = self.clone();
        tokio::spawn(async move {
            let _operation = coordinator.operation_gate(harness).lock_owned().await;
            // A policy change, disable, dismissal, or another update may have
            // won while this task waited for the provider operation slot.
            if coordinator.automatic_update_ready(harness)
                && let Err(error) = coordinator.apply_locked(harness, true).await
            {
                tracing::warn!(?harness, %error, "automatic harness update failed");
            }
        });
    }

    async fn check_one_inner(&self, harness: HarnessId) -> Result<(), String> {
        // Checks and activation refreshes must never overwrite a live
        // mutation phase (especially Installing, which is non-interruptible).
        if self.is_mutating(harness) {
            return Ok(());
        }
        let _operation = self.operation_gate(harness).lock_owned().await;
        // The mutation may have claimed this harness while the check was
        // waiting for its per-provider operation slot.
        if self.is_mutating(harness) {
            return Ok(());
        }
        if self.settle_if_unmonitored(harness) {
            return Ok(());
        }
        let _check_slot = self
            .inner
            .check_slots
            .acquire()
            .await
            .map_err(|_| "agent update checker is shutting down".to_string())?;
        if self.settle_if_unmonitored(harness) {
            return Ok(());
        }
        self.mutate(harness, |status| {
            status.phase = HarnessUpdatePhase::Checking;
            status.progress = None;
            status.error = None;
        });
        let executable = match self.executable(harness) {
            Ok(path) => path,
            Err(error) => {
                if self.settle_if_unmonitored(harness) {
                    return Ok(());
                }
                return self.fail_check(harness, error);
            }
        };
        let spec = provider(harness);
        let version_lease = self.inner.registry.execution_lease(harness).await;
        let installed = run_version_command(&executable, spec.version_args).await;
        drop(version_lease);
        let installed = match installed {
            Ok(version) => version,
            Err(error) => {
                if self.settle_if_unmonitored(harness) {
                    return Ok(());
                }
                return self.fail_check(harness, error);
            }
        };
        let source = classify_source(&executable);
        let can_apply = can_apply_update(harness, &executable);
        let manual_command = (!can_apply)
            .then(|| {
                claude_package_manager_command_for(harness, &executable)
                    .unwrap_or_else(|| provider(harness).manual_command.to_string())
            })
            .filter(|command| !command.is_empty());
        if self.settle_if_unmonitored(harness) {
            return Ok(());
        }
        let latest = match spec.latest {
            LatestSource::Claude => {
                let channel = if let Some(channel) = claude_cask_channel(&executable) {
                    Some(channel)
                } else {
                    // Let the CLI resolve user, project, MDM and server-managed
                    // policy. Guessing from a subset of its files can advertise
                    // a release that its updater will never install.
                    let lease = self.inner.registry.execution_lease(harness).await;
                    let diagnostic =
                        run_command_output(&executable, &["doctor"], COMMAND_TIMEOUT).await;
                    drop(lease);
                    diagnostic
                        .ok()
                        .and_then(|output| parse_claude_release_channel(&output))
                };
                self.mutate(harness, |status| {
                    status.channel = channel.map(str::to_owned)
                });
                match channel {
                    Some(channel) => self
                        .npm_release("@anthropic-ai/claude-code", channel)
                        .await
                        .map(UpdateCheck::Version),
                    // Older doctor commands can require a terminal or omit the
                    // channel. Keep explicit checks/updates available, without
                    // claiming a release or scheduling automatic installation.
                    None => Ok(UpdateCheck::Manual),
                }
            }
            LatestSource::Npm(package) => self.npm_latest(package).await.map(UpdateCheck::Version),
            LatestSource::Opencode => self
                .npm_latest(opencode_release_package(&installed))
                .await
                .map(UpdateCheck::Version),
            LatestSource::Github {
                repository,
                tag_prefix,
            } => self
                .github_latest(repository, tag_prefix)
                .await
                .map(UpdateCheck::Version),
            LatestSource::Command(args) => {
                let lease = self.inner.registry.execution_lease(harness).await;
                let result = run_command_output(&executable, args, COMMAND_TIMEOUT)
                    .await
                    .and_then(|output| {
                        extract_latest_version(&output)
                            .ok_or_else(|| "command returned no recognizable version".into())
                    });
                drop(lease);
                result.map(UpdateCheck::Version)
            }
            LatestSource::Hermes => {
                let lease = self.inner.registry.execution_lease(harness).await;
                let result =
                    run_command_output(&executable, &["update", "--check"], COMMAND_TIMEOUT)
                        .await
                        .and_then(|output| parse_hermes_update_check(&output));
                drop(lease);
                result
            }
            LatestSource::Manual => Ok(UpdateCheck::Manual),
        };
        if self.settle_if_unmonitored(harness) {
            return Ok(());
        }
        let (latest, available) = match latest {
            Ok(UpdateCheck::Version(version)) => {
                let dismissed = lock(&self.inner.prefs)
                    .dismissed_versions
                    .get(&harness)
                    .is_some_and(|dismissed| dismissed == &version);
                let available = version_is_newer(&version, &installed) && !dismissed;
                (Some(version), available)
            }
            // Hermes tracks Git commits, which can advance without changing
            // its CLI version. Preserve its verdict without inventing a release.
            Ok(UpdateCheck::Available) => (None, true),
            Ok(UpdateCheck::Current) => (None, false),
            Ok(UpdateCheck::Manual) => {
                let registry = self.inner.registry.clone();
                self.mutate(harness, |status| {
                    status.installed_version = Some(installed);
                    status.latest_version = None;
                    status.channel = None;
                    status.source = source;
                    status.can_apply = can_apply;
                    status.manual_command =
                        manual_command.or_else(|| Some(spec.manual_command.into()));
                    status.phase = if status.policy == HarnessUpdatePolicy::Off
                        || !registry.enabled_set().contains(&harness)
                    {
                        HarnessUpdatePhase::Dormant
                    } else {
                        HarnessUpdatePhase::ManualActionRequired
                    };
                    status.checked_at = Some(now_ms());
                    status.error = None;
                });
                return Ok(());
            }

            Err(error) => {
                return self.fail_check_with_installed(harness, installed, source, error);
            }
        };
        let registry = self.inner.registry.clone();
        self.mutate(harness, |status| {
            status.installed_version = Some(installed);
            status.latest_version = latest;
            status.source = source;
            status.can_apply = can_apply;
            status.manual_command = manual_command;
            status.phase = if status.policy == HarnessUpdatePolicy::Off
                || !registry.enabled_set().contains(&harness)
            {
                HarnessUpdatePhase::Dormant
            } else if available {
                HarnessUpdatePhase::Available
            } else {
                HarnessUpdatePhase::Current
            };
            status.checked_at = Some(now_ms());
            status.error = None;
        });
        Ok(())
    }

    /// Apply one known release. Cancellation is honored while waiting for the
    /// exclusive lease and before mutation; once the vendor updater starts,
    /// the installation phase is intentionally non-interruptible. The owned
    /// task is detached from the requesting RPC so closing Settings, losing a
    /// relay, or timing out a client cannot drop an updater mid-mutation.
    pub async fn apply(&self, harness: HarnessId) -> Result<String, String> {
        let coordinator = self.clone();
        tokio::spawn(async move { coordinator.apply_inner(harness).await })
            .await
            .map_err(|error| format!("agent update task failed: {error}"))?
    }

    async fn apply_inner(&self, harness: HarnessId) -> Result<String, String> {
        // A provider probe and mutation must never overlap: a late check
        // result could otherwise overwrite Installing/Verifying state or
        // inspect a binary while its owner is replacing it.
        let _operation = tokio::select! {
            biased;
            _ = self.inner.shutdown.cancelled() => return Err("update cancelled".into()),
            operation = self.operation_gate(harness).lock_owned() => operation,
        };
        self.apply_locked(harness, false).await
    }

    /// Caller holds the provider operation lock through verification.
    async fn apply_locked(&self, harness: HarnessId, automatic: bool) -> Result<String, String> {
        if self.inner.shutdown.is_cancelled() {
            return Err("update cancelled".into());
        }
        let current = self
            .snapshot()
            .into_iter()
            .find(|status| status.harness == harness)
            .ok_or_else(|| "unknown harness".to_string())?;
        if !matches!(
            current.phase,
            HarnessUpdatePhase::Available | HarnessUpdatePhase::ManualActionRequired
        ) {
            return Err("no applicable harness update".into());
        }
        let executable = self.executable(harness)?;
        let plan = update_plan(harness, &executable)?;
        // A request queued behind a check must inherit shutdown even if it
        // reaches this point after shutdown's cancellation-map snapshot.
        let cancel = self.inner.shutdown.child_token();
        {
            let mut cancellations = lock(&self.inner.cancellations);
            if cancellations.contains_key(&harness) {
                return Err("an update is already in progress".into());
            }
            if automatic && self.policy(harness) != HarnessUpdatePolicy::AutoWhenIdle {
                return Err("update cancelled".into());
            }
            cancellations.insert(
                harness,
                ActiveUpdate {
                    cancel: cancel.clone(),
                    automatic,
                    previous_phase: current.phase,
                },
            );
        }
        self.inner.registry.begin_update(harness);
        let mut intent = UpdateIntentGuard {
            coordinator: self.clone(),
            harness,
            complete: false,
        };
        self.mutate(harness, |status| {
            status.phase = HarnessUpdatePhase::WaitingForIdle;
            status.progress = None;
            status.error = None;
        });
        let lease = tokio::select! {
            _ = cancel.cancelled() => {
                self.finish_cancelled(harness);
                intent.finish();
                return Err("update cancelled".into());
            }
            lease = self.inner.registry.update_lease(harness) => lease,
        };
        if cancel.is_cancelled() || !self.inner.registry.enabled_set().contains(&harness) {
            drop(lease);
            self.finish_cancelled(harness);
            intent.finish();
            return Err("update cancelled".into());
        }
        self.mutate(harness, |status| {
            status.phase = HarnessUpdatePhase::Preparing
        });
        tokio::task::yield_now().await;
        if cancel.is_cancelled() || !self.inner.registry.enabled_set().contains(&harness) {
            drop(lease);
            self.finish_cancelled(harness);
            intent.finish();
            return Err("update cancelled".into());
        }
        let applied = match plan {
            UpdatePlan::Command { executable, args } => {
                match self.begin_install(harness, &cancel) {
                    Ok(()) => run_command(&executable, args, UPDATE_TIMEOUT).await,
                    Err(error) => Err(error),
                }
            }
            UpdatePlan::CodexStandalone(install) => {
                self.install_codex_standalone(harness, &current, install, &cancel)
                    .await
            }
        };
        if let Err(error) = applied {
            drop(lease);
            if cancel.is_cancelled() {
                self.finish_cancelled(harness);
            } else {
                self.finish_failed_update(harness, error.clone());
            }
            intent.finish();
            return Err(error);
        }
        self.mutate(harness, |status| status.progress = None);
        self.mutate(harness, |status| {
            status.phase = HarnessUpdatePhase::Verifying
        });
        let verified = run_version_command(&executable, provider(harness).version_args).await;
        let result = match verified {
            Ok(version) => {
                let expected = current.latest_version.as_deref();
                if expected.is_some_and(|latest| version_is_newer(latest, &version)) {
                    Err(format!(
                        "verification returned {version}, older than expected {}",
                        expected.unwrap_or_default()
                    ))
                } else {
                    lock(&self.inner.prefs).dismissed_versions.remove(&harness);
                    self.persist_preferences();
                    self.mutate(harness, |status| {
                        status.installed_version = Some(version.clone());
                        status.phase = HarnessUpdatePhase::Updated;
                        status.checked_at = Some(now_ms());
                        status.error = None;
                    });
                    Ok(version)
                }
            }
            Err(error) => Err(format!("post-update verification failed: {error}")),
        };
        drop(lease);
        lock(&self.inner.cancellations).remove(&harness);
        self.inner.registry.end_update(harness);
        // Updated was published before the operation left the active set.
        // Wake shutdown even if it already consumed that final status frame.
        self.inner.status_tx.send_modify(|_| {});
        if let Err(error) = &result {
            self.fail(harness, error.clone()).ok();
        } else {
            let coordinator = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(4)).await;
                coordinator.mutate(harness, |status| {
                    if status.phase == HarnessUpdatePhase::Updated {
                        status.phase = if status.policy == HarnessUpdatePolicy::Off {
                            HarnessUpdatePhase::Dormant
                        } else {
                            HarnessUpdatePhase::Current
                        };
                    }
                });
            });
        }
        intent.finish();
        result
    }

    /// Serialize the final cancellation check and installation commit with
    /// `cancel`: once cancellation is accepted, mutation cannot begin.
    fn begin_install(&self, harness: HarnessId, cancel: &CancellationToken) -> Result<(), String> {
        let cancellations = lock(&self.inner.cancellations);
        if cancellations
            .get(&harness)
            .is_some_and(|update| update.automatic)
            && self.policy(harness) != HarnessUpdatePolicy::AutoWhenIdle
        {
            cancel.cancel();
        }
        if cancel.is_cancelled() || !self.inner.registry.enabled_set().contains(&harness) {
            return Err("update cancelled".into());
        }
        self.mutate(harness, |status| {
            status.phase = HarnessUpdatePhase::Installing;
            status.progress = None;
        });
        Ok(())
    }

    pub fn cancel(&self, harness: HarnessId) -> bool {
        let cancellations = lock(&self.inner.cancellations);
        if !matches!(
            self.status(harness).phase,
            HarnessUpdatePhase::WaitingForIdle
                | HarnessUpdatePhase::Preparing
                | HarnessUpdatePhase::Downloading
        ) {
            return false;
        }
        cancellations
            .get(&harness)
            .map(|update| update.cancel.cancel())
            .is_some()
    }

    pub fn dismiss(&self, harness: HarnessId, version: Option<String>) -> HarnessUpdateStatus {
        let version = version.or_else(|| {
            self.snapshot()
                .into_iter()
                .find(|status| status.harness == harness)
                .and_then(|status| status.latest_version)
        });
        if let Some(version) = version {
            lock(&self.inner.prefs)
                .dismissed_versions
                .insert(harness, version);
            self.persist_preferences();
            self.mutate(harness, |status| {
                if status.phase == HarnessUpdatePhase::Available {
                    status.phase = HarnessUpdatePhase::Current;
                }
            });
        }
        self.status(harness)
    }

    pub fn set_policy(
        &self,
        harness: HarnessId,
        policy: HarnessUpdatePolicy,
    ) -> HarnessUpdateStatus {
        let current = self.status(harness);
        let was_applicable = current.phase == HarnessUpdatePhase::Available && current.can_apply;
        {
            // Policy selection and the installation boundary share the same
            // lock: a queued automatic request cannot outlive opting out.
            let active = lock(&self.inner.cancellations);
            lock(&self.inner.prefs).policies.insert(harness, policy);
            if let Some(update) = active.get(&harness)
                && (policy == HarnessUpdatePolicy::Off
                    || (update.automatic && policy != HarnessUpdatePolicy::AutoWhenIdle))
                && !matches!(
                    self.status(harness).phase,
                    HarnessUpdatePhase::Installing
                        | HarnessUpdatePhase::Verifying
                        | HarnessUpdatePhase::Updated
                )
            {
                update.cancel.cancel();
            }
            self.mutate(harness, |status| {
                status.policy = policy;
                if policy == HarnessUpdatePolicy::Off && !active.contains_key(&harness) {
                    status.phase = HarnessUpdatePhase::Dormant;
                    status.error = None;
                }
            });
        }
        self.persist_preferences();
        if policy == HarnessUpdatePolicy::AutoWhenIdle && was_applicable {
            self.schedule_automatic_update(harness);
        } else if policy != HarnessUpdatePolicy::Off {
            let coordinator = self.clone();
            tokio::spawn(async move {
                let _ = coordinator.check_one(harness).await;
            });
        }
        self.status(harness)
    }

    pub fn refresh_enabled(&self) {
        // Cancel before scheduling checks: checks intentionally skip a provider
        // that already owns its operation slot while waiting for an active run.
        let enabled = self.inner.registry.enabled_set();
        for harness in &self.inner.order {
            if !enabled.contains(harness) {
                self.cancel(*harness);
            }
        }
        let coordinator = self.clone();
        tokio::spawn(async move {
            coordinator.check_all().await;
        });
    }

    pub async fn shutdown(&self) {
        self.inner.shutdown.cancel();
        for update in lock(&self.inner.cancellations).values() {
            update.cancel.cancel();
        }
        let worker = lock(&self.inner.worker).take();
        if let Some(worker) = worker {
            let _ = worker.await;
        }
        // Waiting/preparing operations observe cancellation immediately;
        // installing/verifying operations are deliberately non-interruptible
        // and must settle before the engine tears down their environment.
        let mut status = self.watch();
        let settle = async {
            while !lock(&self.inner.cancellations).is_empty() {
                if status.changed().await.is_err() {
                    break;
                }
            }
        };
        if tokio::time::timeout(UPDATE_TIMEOUT, settle).await.is_err() {
            tracing::warn!("timed out waiting for harness update to settle during shutdown");
        }
    }

    fn policy(&self, harness: HarnessId) -> HarnessUpdatePolicy {
        lock(&self.inner.prefs)
            .policies
            .get(&harness)
            .copied()
            .unwrap_or_default()
    }

    fn status(&self, harness: HarnessId) -> HarnessUpdateStatus {
        lock(&self.inner.statuses)
            .get(&harness)
            .cloned()
            .unwrap_or_else(|| HarnessUpdateStatus {
                harness,
                installed_version: None,
                latest_version: None,
                channel: Some("stable".into()),
                source: HarnessInstallSource::Unknown,
                policy: self.policy(harness),
                phase: HarnessUpdatePhase::Dormant,
                progress: None,
                checked_at: None,
                error: None,
                can_apply: provider(harness).update_args.is_some(),
                manual_command: None,
            })
    }

    fn mutate(&self, harness: HarnessId, change: impl FnOnce(&mut HarnessUpdateStatus)) {
        let mut statuses = lock(&self.inner.statuses);
        let snapshot = {
            let policy = self.policy(harness);
            let status = statuses
                .entry(harness)
                .or_insert_with(|| HarnessUpdateStatus {
                    harness,
                    installed_version: None,
                    latest_version: None,
                    channel: Some("stable".into()),
                    source: HarnessInstallSource::Unknown,
                    policy,
                    phase: HarnessUpdatePhase::Dormant,
                    progress: None,
                    checked_at: None,
                    error: None,
                    can_apply: provider(harness).update_args.is_some(),
                    manual_command: None,
                });
            // Preferences are the durable authority. Refresh the copy while
            // holding the status lock so a check result and a policy change
            // have one deterministic order instead of resurrecting an
            // Available/Failed phase after Updates: Off.
            status.policy = policy;
            change(status);
            ordered_snapshot(&self.inner.order, &statuses)
        };
        // Keep publication in the same critical section as the state change.
        // Otherwise concurrent providers can publish full snapshots backwards.
        self.inner.status_tx.send_replace(snapshot);
    }

    fn executable(&self, harness: HarnessId) -> Result<PathBuf, String> {
        self.inner
            .registry
            .resolve(harness)
            .map_err(|error| error.to_string())?
            .executable_path()
            .ok_or_else(|| "agent CLI executable is unavailable".into())
    }

    fn operation_gate(&self, harness: HarnessId) -> Arc<tokio::sync::Mutex<()>> {
        lock(&self.inner.operation_gates)
            .entry(harness)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    fn settle_if_unmonitored(&self, harness: HarnessId) -> bool {
        let monitored = self.policy(harness) != HarnessUpdatePolicy::Off
            && self.inner.registry.enabled_set().contains(&harness);
        if monitored {
            return false;
        }
        self.mutate(harness, |status| {
            status.phase = HarnessUpdatePhase::Dormant;
            status.progress = None;
            status.error = None;
        });
        true
    }

    async fn npm_latest(&self, package: &str) -> Result<String, String> {
        self.npm_release(package, "latest").await
    }

    async fn npm_release(&self, package: &str, channel: &str) -> Result<String, String> {
        let encoded = package.replace('/', "%2f");
        let url = format!("https://registry.npmjs.org/{encoded}/{channel}");
        let response = self
            .inner
            .client
            .get(url)
            .send()
            .await
            .map_err(|error| format!("latest-version check failed: {error}"))?
            .error_for_status()
            .map_err(|error| format!("latest-version check failed: {error}"))?;
        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|error| format!("latest-version response was invalid: {error}"))?;
        json.get("version")
            .and_then(serde_json::Value::as_str)
            .filter(|version| version_numbers(version).is_some())
            .map(str::to_owned)
            .ok_or_else(|| "latest-version response contained no version".into())
    }

    async fn github_latest(&self, repository: &str, tag_prefix: &str) -> Result<String, String> {
        let url = format!("https://api.github.com/repos/{repository}/releases/latest");
        let response = self
            .inner
            .client
            .get(url)
            .send()
            .await
            .map_err(|error| format!("latest-version check failed: {error}"))?
            .error_for_status()
            .map_err(|error| format!("latest-version check failed: {error}"))?;
        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|error| format!("latest-version response was invalid: {error}"))?;
        json.get("tag_name")
            .and_then(serde_json::Value::as_str)
            .and_then(|tag| tag.strip_prefix(tag_prefix))
            .filter(|version| version_numbers(version).is_some())
            .map(str::to_owned)
            .ok_or_else(|| "latest-version response contained no version".into())
    }

    async fn github_release_asset(
        &self,
        repository: &str,
        tag: &str,
        asset_name: &str,
    ) -> Result<ReleaseAsset, String> {
        let url = format!("https://api.github.com/repos/{repository}/releases/tags/{tag}");
        let response = self
            .inner
            .client
            .get(url)
            .send()
            .await
            .map_err(|error| format!("release lookup failed: {error}"))?
            .error_for_status()
            .map_err(|error| format!("release lookup failed: {error}"))?;
        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|error| format!("release response was invalid: {error}"))?;
        let asset = json
            .get("assets")
            .and_then(serde_json::Value::as_array)
            .and_then(|assets| {
                assets.iter().find(|asset| {
                    asset.get("name").and_then(serde_json::Value::as_str) == Some(asset_name)
                })
            })
            .ok_or_else(|| format!("release does not contain {asset_name}"))?;
        let url = asset
            .get("browser_download_url")
            .and_then(serde_json::Value::as_str)
            .filter(|url| url.starts_with("https://github.com/openai/codex/releases/download/"))
            .ok_or_else(|| "release asset URL was missing or untrusted".to_string())?;
        let digest = asset
            .get("digest")
            .and_then(serde_json::Value::as_str)
            .and_then(|digest| digest.strip_prefix("sha256:"))
            .filter(|digest| digest.len() == 64 && digest.chars().all(|ch| ch.is_ascii_hexdigit()))
            .ok_or_else(|| "release asset has no SHA-256 digest".to_string())?;
        Ok(ReleaseAsset {
            url: url.to_string(),
            digest: digest.to_ascii_lowercase(),
            size: asset.get("size").and_then(serde_json::Value::as_u64),
        })
    }

    async fn install_codex_standalone(
        &self,
        harness: HarnessId,
        current: &HarnessUpdateStatus,
        install: CodexStandaloneInstall,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        let version = current
            .latest_version
            .as_deref()
            .filter(|version| safe_component(version))
            .ok_or_else(|| "Codex release version is unavailable".to_string())?;
        if !safe_component(&install.target) {
            return Err("Codex standalone target is invalid".into());
        }
        let tag = format!("rust-v{version}");
        let asset_name = format!("codex-package-{}.tar.gz", install.target);
        let asset = cancellable(
            cancel,
            self.github_release_asset("openai/codex", &tag, &asset_name),
        )
        .await?;
        let _install_lock = InstallFileLock::acquire(&install.root)?;
        let releases = install.root.join("releases");
        let nonce = uuid::Uuid::new_v4();
        let archive = install.root.join(format!(".zeron-codex-{nonce}.tar.gz"));
        let staging = releases.join(format!(".{version}-{}-{nonce}.partial", install.target));
        std::fs::create_dir(&staging)
            .map_err(|error| format!("could not stage Codex update: {error}"))?;
        let mut cleanup = CodexStageCleanup::new(archive.clone(), staging.clone());

        self.mutate(harness, |status| {
            status.phase = HarnessUpdatePhase::Downloading;
            status.progress = Some(HarnessUpdateProgress {
                completed_bytes: Some(0),
                total_bytes: asset.size,
                message: Some(format!("Downloading Codex {version}")),
            });
        });
        let (completed, total) = cancellable(
            cancel,
            self.download_codex_archive(harness, version, &asset, &archive),
        )
        .await?;
        if cancel.is_cancelled() {
            return Err("update cancelled".into());
        }
        self.mutate(harness, |status| {
            status.progress = Some(HarnessUpdateProgress {
                completed_bytes: Some(completed),
                total_bytes: total,
                message: Some("Unpacking verified update".into()),
            });
        });
        let tar = [Path::new("/usr/bin/tar"), Path::new("/bin/tar")]
            .into_iter()
            .find(|path| path.is_file())
            .ok_or_else(|| "system tar is unavailable".to_string())?;
        let archive_arg = archive.to_string_lossy();
        let staging_arg = staging.to_string_lossy();
        run_command(
            tar,
            &["-xzf", archive_arg.as_ref(), "-C", staging_arg.as_ref()],
            UPDATE_TIMEOUT,
        )
        .await?;
        validate_codex_package(&staging, version, &install.target)?;
        self.begin_install(harness, cancel)?;
        let destination = releases.join(format!("{version}-{}", install.target));
        if destination.exists() {
            validate_codex_package(&destination, version, &install.target)?;
        } else {
            std::fs::rename(&staging, &destination)
                .map_err(|error| format!("could not install Codex release: {error}"))?;
            cleanup.release_staging();
        }
        activate_codex_release(&install.root, &destination, nonce)?;
        Ok(())
    }

    /// Download and validate bytes only; activation happens after cancellation
    /// has been checked by the caller. Dropping this future closes the request
    /// and archive file, and the install's stage guard removes partial files.
    async fn download_codex_archive(
        &self,
        harness: HarnessId,
        version: &str,
        asset: &ReleaseAsset,
        archive: &Path,
    ) -> Result<(u64, Option<u64>), String> {
        let response = self
            .inner
            .client
            .get(&asset.url)
            .timeout(UPDATE_TIMEOUT)
            .send()
            .await
            .map_err(|error| format!("Codex download failed: {error}"))?
            .error_for_status()
            .map_err(|error| format!("Codex download failed: {error}"))?;
        let total = response.content_length().or(asset.size);
        let mut stream = response.bytes_stream();
        let mut file = tokio::fs::File::create(&archive)
            .await
            .map_err(|error| format!("could not create Codex update archive: {error}"))?;
        let mut hasher = Sha256::new();
        let mut completed = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| format!("Codex download failed: {error}"))?;
            file.write_all(&chunk)
                .await
                .map_err(|error| format!("could not write Codex update archive: {error}"))?;
            hasher.update(&chunk);
            completed = completed.saturating_add(chunk.len() as u64);
            self.mutate(harness, |status| {
                status.progress = Some(HarnessUpdateProgress {
                    completed_bytes: Some(completed),
                    total_bytes: total,
                    message: Some(format!("Downloading Codex {version}")),
                });
            });
        }
        file.flush()
            .await
            .map_err(|error| format!("could not finish Codex update archive: {error}"))?;
        file.sync_all()
            .await
            .map_err(|error| format!("could not sync Codex update archive: {error}"))?;
        drop(file);
        if asset.size.is_some_and(|expected| completed != expected) {
            return Err(format!(
                "Codex update download was incomplete ({completed} of {} bytes)",
                asset.size.unwrap_or_default()
            ));
        }
        let digest = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if digest != asset.digest {
            return Err("Codex update archive failed SHA-256 verification".into());
        }
        Ok((completed, total))
    }

    fn is_mutating(&self, harness: HarnessId) -> bool {
        lock(&self.inner.cancellations).contains_key(&harness)
    }

    fn fail(&self, harness: HarnessId, error: String) -> Result<(), String> {
        self.mutate(harness, |status| {
            status.phase = HarnessUpdatePhase::Failed;
            status.checked_at = Some(now_ms());
            status.error = Some(HarnessUpdateFailure {
                message: error.clone(),
                retryable: true,
            });
        });
        Err(error)
    }

    fn fail_check(&self, harness: HarnessId, error: String) -> Result<(), String> {
        let registry = self.inner.registry.clone();
        self.mutate(harness, |status| {
            if status.policy == HarnessUpdatePolicy::Off
                || !registry.enabled_set().contains(&harness)
            {
                status.phase = HarnessUpdatePhase::Dormant;
                status.error = None;
            } else {
                status.phase = HarnessUpdatePhase::Failed;
                status.checked_at = Some(now_ms());
                status.error = Some(HarnessUpdateFailure {
                    message: error.clone(),
                    retryable: true,
                });
            }
        });
        Err(error)
    }

    fn fail_check_with_installed(
        &self,
        harness: HarnessId,
        installed: String,
        source: HarnessInstallSource,
        error: String,
    ) -> Result<(), String> {
        self.mutate(harness, |status| {
            status.installed_version = Some(installed);
            status.source = source;
        });
        self.fail_check(harness, error)
    }

    fn finish_cancelled(&self, harness: HarnessId) {
        let previous_phase = lock(&self.inner.cancellations)
            .remove(&harness)
            .map(|update| update.previous_phase)
            .unwrap_or(HarnessUpdatePhase::ManualActionRequired);
        self.inner.registry.end_update(harness);
        let enabled = self.inner.registry.enabled_set().contains(&harness);
        self.mutate(harness, |status| {
            status.progress = None;
            status.phase = if !enabled || status.policy == HarnessUpdatePolicy::Off {
                HarnessUpdatePhase::Dormant
            } else {
                previous_phase
            };
            status.error = None;
        });
    }

    fn finish_failed_update(&self, harness: HarnessId, error: String) {
        lock(&self.inner.cancellations).remove(&harness);
        self.inner.registry.end_update(harness);
        self.fail(harness, error).ok();
    }

    fn persist_preferences(&self) {
        // Keep snapshot order and replacement order identical. Every writer
        // shares this lock and temporary path, including successful updates.
        let prefs = lock(&self.inner.prefs);
        let json = match serde_json::to_string_pretty(&*prefs) {
            Ok(json) => json,
            Err(error) => {
                tracing::warn!(%error, "harness update preferences serialize failed");
                return;
            }
        };
        let temp = self.inner.prefs_path.with_extension("json.tmp");
        if let Err(error) = std::fs::write(&temp, json)
            .and_then(|()| std::fs::rename(&temp, &self.inner.prefs_path))
        {
            tracing::warn!(%error, "harness update preferences save failed");
        }
    }
}

/// Only wrap pre-install work: mutation and verification remain non-interruptible.
async fn cancellable<T>(
    cancel: &CancellationToken,
    operation: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err("update cancelled".into()),
        result = operation => result,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn ordered_snapshot(
    order: &[HarnessId],
    statuses: &HashMap<HarnessId, HarnessUpdateStatus>,
) -> Vec<HarnessUpdateStatus> {
    order
        .iter()
        .filter_map(|id| statuses.get(id).cloned())
        .collect()
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+'))
}

/// Recognize only the canonical layout produced by the official Codex
/// standalone installer. A random executable containing "codex" in its path
/// must remain manual rather than becoming an update target.
#[cfg(unix)]
fn codex_standalone_install(executable: &Path) -> Option<CodexStandaloneInstall> {
    let executable = std::fs::canonicalize(executable).ok()?;
    if executable.file_name()?.to_str()? != "codex"
        || executable.parent()?.file_name()?.to_str()? != "bin"
    {
        return None;
    }
    let release = executable.parent()?.parent()?;
    let releases = release.parent()?;
    if releases.file_name()?.to_str()? != "releases" {
        return None;
    }
    let root = releases.parent()?.to_path_buf();
    let active = std::fs::canonicalize(root.join("current")).ok()?;
    if active != release {
        return None;
    }
    let manifest = read_codex_manifest(release).ok()?;
    if manifest.layout_version != 1
        || manifest.variant != "codex"
        || manifest.entrypoint != "bin/codex"
        || !safe_component(&manifest.version)
        || !safe_component(&manifest.target)
    {
        return None;
    }
    Some(CodexStandaloneInstall {
        root,
        target: manifest.target,
    })
}

#[cfg(not(unix))]
fn codex_standalone_install(_executable: &Path) -> Option<CodexStandaloneInstall> {
    None
}

fn read_codex_manifest(directory: &Path) -> Result<CodexPackageManifest, String> {
    let bytes = std::fs::read(directory.join("codex-package.json"))
        .map_err(|error| format!("could not read Codex package manifest: {error}"))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("Codex package manifest was invalid: {error}"))
}

fn validate_codex_package(directory: &Path, version: &str, target: &str) -> Result<(), String> {
    let manifest = read_codex_manifest(directory)?;
    if manifest.layout_version != 1
        || manifest.version != version
        || manifest.target != target
        || manifest.variant != "codex"
        || manifest.entrypoint != "bin/codex"
        || manifest.resources_dir != "codex-resources"
        || manifest.path_dir != "codex-path"
    {
        return Err("Codex update package metadata did not match the requested release".into());
    }
    let entrypoint = directory.join(&manifest.entrypoint);
    let canonical_directory = std::fs::canonicalize(directory)
        .map_err(|error| format!("could not inspect Codex update package: {error}"))?;
    let canonical_entrypoint = std::fs::canonicalize(&entrypoint)
        .map_err(|error| format!("Codex update package has no executable: {error}"))?;
    if !canonical_entrypoint.starts_with(&canonical_directory) || !entrypoint.is_file() {
        return Err("Codex update package entrypoint escaped the release directory".into());
    }
    for required in [&manifest.resources_dir, &manifest.path_dir] {
        let path = directory.join(required);
        let canonical = std::fs::canonicalize(&path)
            .map_err(|error| format!("Codex update package is incomplete: {error}"))?;
        if !path.is_dir() || !canonical.starts_with(&canonical_directory) {
            return Err("Codex update package contains an unsafe resource path".into());
        }
    }
    Ok(())
}

#[cfg(unix)]
fn activate_codex_release(
    root: &Path,
    destination: &Path,
    nonce: uuid::Uuid,
) -> Result<(), String> {
    use std::os::unix::fs::symlink;

    let canonical_root = std::fs::canonicalize(root)
        .map_err(|error| format!("could not inspect Codex installation: {error}"))?;
    let canonical_destination = std::fs::canonicalize(destination)
        .map_err(|error| format!("could not inspect installed Codex release: {error}"))?;
    if !canonical_destination.starts_with(canonical_root.join("releases")) {
        return Err("Codex release destination escaped the installation".into());
    }
    let temporary = root.join(format!(".current.zeron-{nonce}"));
    symlink(&canonical_destination, &temporary)
        .map_err(|error| format!("could not stage Codex activation: {error}"))?;
    if let Err(error) = std::fs::rename(&temporary, root.join("current")) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("could not activate Codex release: {error}"));
    }
    if let Ok(directory) = File::open(root) {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(not(unix))]
fn activate_codex_release(
    _root: &Path,
    _destination: &Path,
    _nonce: uuid::Uuid,
) -> Result<(), String> {
    Err("automatic Codex standalone updates are not supported on this platform".into())
}

fn classify_source(path: &Path) -> HarnessInstallSource {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = format!("{}\n{}", path.display(), canonical.display()).to_ascii_lowercase();
    if text.contains("/.cargo/bin/") {
        HarnessInstallSource::Cargo
    } else if text.contains("node_modules")
        || text.contains("/.nvm/")
        || text.contains("/.volta/")
        || text.contains("/.bun/")
        || text.contains("/pnpm/")
    {
        HarnessInstallSource::Npm
    } else if text.contains("homebrew") || text.contains("/cellar/") || text.contains("/caskroom/")
    {
        HarnessInstallSource::Homebrew
    } else {
        HarnessInstallSource::Vendor
    }
}

fn claude_package_manager_command_for(harness: HarnessId, path: &Path) -> Option<String> {
    (harness == HarnessId::ClaudeCode)
        .then(|| claude_package_manager_command(path))
        .flatten()
}

fn claude_package_manager_command(path: &Path) -> Option<String> {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = format!("{}\n{}", path.display(), canonical.display())
        .replace('\\', "/")
        .to_ascii_lowercase();
    if let Some(channel) = claude_cask_channel(path) {
        Some(if channel == "latest" {
            "brew upgrade claude-code@latest".into()
        } else {
            "brew upgrade claude-code".into()
        })
    } else if text.contains("/winget/") || text.contains("/microsoft/winget/") {
        Some("winget upgrade Anthropic.ClaudeCode".into())
    } else if canonical.starts_with("/usr/bin")
        || canonical.starts_with("/nix/store")
        || canonical.starts_with("/snap")
    {
        Some("Update Claude Code with the system package manager that installed it".into())
    } else {
        None
    }
}

/// Only actual cask paths establish Homebrew ownership. An npm install
/// under /opt/homebrew/lib/node_modules belongs to npm, not a Claude cask.
fn claude_cask_channel(path: &Path) -> Option<&'static str> {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = canonical
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    for owner in ["/caskroom/", "/cellar/"] {
        if text.contains(&format!("{owner}claude-code@latest/")) {
            return Some("latest");
        }
        if text.contains(&format!("{owner}claude-code/")) {
            return Some("stable");
        }
    }
    None
}

fn parse_claude_release_channel(output: &str) -> Option<&'static str> {
    output.lines().find_map(
        |line| match line.trim().strip_prefix("Auto-update channel:")?.trim() {
            "stable" => Some("stable"),
            "latest" => Some("latest"),
            _ => None,
        },
    )
}

fn parse_hermes_update_check(output: &str) -> Result<UpdateCheck, String> {
    for line in output.lines() {
        let line = line.trim_start_matches(|ch: char| !ch.is_ascii_alphanumeric());
        if line == "Already up to date." {
            return Ok(UpdateCheck::Current);
        }
        if line.starts_with("Update available: ") && line.contains(" behind ")
            || line.starts_with("Update available (behind ") && line.ends_with(").")
        {
            return Ok(UpdateCheck::Available);
        }
    }
    Err("Hermes update check returned no recognizable verdict".into())
}

async fn run_version_command(executable: &Path, args: &[&str]) -> Result<String, String> {
    let output = run_command_output(executable, args, COMMAND_TIMEOUT).await?;
    extract_version(&output).ok_or_else(|| "command returned no recognizable version".into())
}

async fn run_command(executable: &Path, args: &[&str], timeout: Duration) -> Result<(), String> {
    run_command_output(executable, args, timeout)
        .await
        .map(drop)
}

/// Unix updaters can delegate installation to npm, pip, or a shell. Own
/// their process group as well as the leader, including on future cancellation.
#[cfg(unix)]
struct UpdateProcessGroup(libc::pid_t);

#[cfg(unix)]
impl Drop for UpdateProcessGroup {
    fn drop(&mut self) {
        // SAFETY: spawn creates a private group whose ID is this child's PID.
        unsafe { libc::kill(-self.0, libc::SIGKILL) };
    }
}

async fn run_command_output(
    executable: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("NO_COLOR", "1");
    zeron_harness::compose_child_path(&mut command, executable);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not run {}: {error}", executable.display()))?;
    #[cfg(unix)]
    let group =
        UpdateProcessGroup(child.id().expect("newly spawned updater has a PID") as libc::pid_t);
    let mut stdout = child.stdout.take().expect("updater stdout is piped");
    let mut stderr = child.stderr.take().expect("updater stderr is piped");
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let result = {
        use tokio::io::AsyncReadExt as _;
        tokio::time::timeout(timeout, async {
            tokio::try_join!(
                child.wait(),
                stdout.read_to_end(&mut stdout_bytes),
                stderr.read_to_end(&mut stderr_bytes),
            )
        })
        .await
    };
    // Stop descendants before returning and releasing the execution lease,
    // including when the leader exited but a descendant kept its pipes open.
    #[cfg(unix)]
    drop(group);
    let status = match result {
        Ok(Ok((status, _, _))) => status,
        error => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(match error {
                Err(_) => format!("{} timed out", executable.display()),
                Ok(Err(error)) => format!("could not run {}: {error}", executable.display()),
                Ok(Ok(_)) => unreachable!(),
            });
        }
    };
    let output = std::process::Output {
        status,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
    };
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success() {
        let detail = if stderr.is_empty() { stdout } else { stderr };
        return Err(format!(
            "{} exited with {}{}",
            executable.display(),
            output.status,
            (!detail.is_empty())
                .then(|| format!(": {detail}"))
                .unwrap_or_default()
        ));
    }
    Ok(if stdout.is_empty() { stderr } else { stdout })
}

fn version_tokens(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|character: char| {
        !(character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '+'))
    })
    .filter(|candidate| version_numbers(candidate).is_some())
    .map(|candidate| candidate.trim_start_matches('v').to_owned())
}

fn extract_version(text: &str) -> Option<String> {
    version_tokens(text).next()
}

/// Update checks name the installed release before the candidate
/// (`available: 1.0.4 -> 1.0.41`), so the release is the final version.
fn extract_latest_version(text: &str) -> Option<String> {
    version_tokens(text).last()
}

fn version_numbers(version: &str) -> Option<Vec<u64>> {
    let version = version.trim().trim_start_matches('v');
    let numeric = version.split(['-', '+']).next()?;
    let values: Vec<u64> = numeric
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    (values.len() >= 2).then_some(values)
}

fn opencode_release_package(installed: &str) -> &'static str {
    // OpenCode v2 ships as a separate CLI package. The v1 self-updater stays
    // on the opencode-ai release line, so advertising a v2 version to it makes
    // a successful no-op look like a failed installation at verification.
    if version_numbers(installed)
        .and_then(|numbers| numbers.first().copied())
        .is_some_and(|major| major >= 2)
    {
        "@opencode/cli"
    } else {
        "opencode-ai"
    }
}

fn version_is_newer(latest: &str, installed: &str) -> bool {
    match (version_numbers(latest), version_numbers(installed)) {
        (Some(mut latest), Some(mut installed)) => {
            let width = latest.len().max(installed.len());
            latest.resize(width, 0);
            installed.resize(width, 0);
            latest > installed
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LatestSource, activate_codex_release, codex_standalone_install, extract_latest_version,
        extract_version, opencode_release_package, provider, validate_codex_package,
        version_is_newer,
    };
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use futures::StreamExt as _;
    use zeron_harness::{Harness, HarnessError, RunControls};
    use zeron_proto::{
        AgentEvent, HarnessId, HarnessUpdatePhase, Model, ReasoningLevel, RunRequest, SteeringMode,
    };

    use crate::registry::HarnessRegistry;

    struct ExecutableHarness(PathBuf, HarnessId);

    #[cfg(unix)]
    #[tokio::test]
    async fn updater_timeout_stops_descendants_before_returning() {
        for leader_exits in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let marker = temp.path().join("mutated");
            let ready = temp.path().join("ready");
            let script = format!(
                "echo $$ > \"$2\"; /bin/sh -c 'sleep 0.5; echo mutated > \"$1\"' sh \"$1\" & {}",
                if leader_exits { "exit 0" } else { "wait" }
            );
            let result = super::run_command_output(
                std::path::Path::new("/bin/sh"),
                &[
                    "-c",
                    &script,
                    "sh",
                    marker.to_str().unwrap(),
                    ready.to_str().unwrap(),
                ],
                Duration::from_millis(250),
            )
            .await;
            assert!(result.unwrap_err().contains("timed out"));
            let pid: i32 = std::fs::read_to_string(&ready)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            // The direct child must already be reaped, not merely signalled.
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
            tokio::time::sleep(Duration::from_millis(500)).await;
            assert!(
                !marker.exists(),
                "installer descendant outlived its execution lease"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_update_probe_stops_descendants() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("mutated");
        let ready = temp.path().join("ready");
        let args = [
            "-c",
            "/bin/sh -c 'echo ready > \"$2\"; sleep 0.5; echo mutated > \"$1\"' sh \"$1\" \"$2\" & wait",
            "sh",
            marker.to_str().unwrap(),
            ready.to_str().unwrap(),
        ];
        let mut probe = Box::pin(super::run_command_output(
            std::path::Path::new("/bin/sh"),
            &args,
            Duration::from_secs(10),
        ));
        tokio::select! {
            result = &mut probe => panic!("probe ended early: {result:?}"),
            _ = async {
                tokio::time::timeout(Duration::from_secs(5), async {
                    while !ready.exists() {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }).await.unwrap();
            } => {}
        }
        drop(probe);
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(
            !marker.exists(),
            "cancelled probe left an installer running"
        );
    }

    #[async_trait]
    impl Harness for ExecutableHarness {
        fn id(&self) -> HarnessId {
            self.1
        }

        fn display_name(&self) -> &str {
            "Fixture"
        }

        fn supports_steering(&self) -> bool {
            false
        }

        fn steering_mode(&self) -> SteeringMode {
            SteeringMode::TurnBoundary
        }

        fn reasoning_levels(&self) -> &[ReasoningLevel] {
            &[]
        }

        fn executable_path(&self) -> Option<PathBuf> {
            Some(self.0.clone())
        }

        async fn models(&self) -> Result<Vec<Model>, HarnessError> {
            Ok(Vec::new())
        }

        async fn run(
            &self,
            _request: RunRequest,
            _controls: RunControls,
        ) -> Result<
            futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>,
            HarnessError,
        > {
            Ok(futures::stream::empty().boxed())
        }
    }

    #[test]
    fn extracts_versions_from_vendor_output() {
        assert_eq!(
            extract_version("codex 0.155.1 (Node.js v22.4.0)"),
            Some("0.155.1".into())
        );
        assert_eq!(
            extract_version("claude 2.1.228 (stable)"),
            Some("2.1.228".into())
        );
        assert_eq!(
            extract_version("latest: v1.4.0\ninstalled: 1.3.2"),
            Some("1.4.0".into())
        );
        assert_eq!(extract_version("no release here"), None);
    }

    #[test]
    fn update_checks_report_the_candidate_after_the_installed_version() {
        assert_eq!(
            extract_latest_version(
                "A new version of Grok Build is available: 1.0.4 -> 1.0.41 [stable]"
            ),
            Some("1.0.41".into())
        );
        assert_eq!(
            extract_latest_version("grok 1.0.41 (d846eb93d9) [stable]"),
            Some("1.0.41".into())
        );
        assert_eq!(extract_latest_version("no release here"), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_update_check_offers_the_newer_release() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("grok");
        std::fs::write(
            &executable,
            "#!/bin/sh\ncase \"$1:$2\" in\n  version:) echo 'grok 1.0.4 (d846eb93d9) [stable]' ;;\n  update:--check) echo 'A new version of Grok Build is available: 1.0.4 -> 1.0.41 [stable]' ;;\n  *) exit 2 ;;\nesac\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(ExecutableHarness(executable, HarnessId::Grok)));
        let coordinator = super::HarnessUpdateCoordinator::new(temp.path(), registry);
        coordinator.check_one(HarnessId::Grok).await.unwrap();
        let status = coordinator.status(HarnessId::Grok);
        assert_eq!(status.phase, HarnessUpdatePhase::Available);
        assert_eq!(status.installed_version.as_deref(), Some("1.0.4"));
        assert_eq!(status.latest_version.as_deref(), Some("1.0.41"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_does_not_wait_for_a_slow_periodic_check() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("grok");
        std::fs::write(&executable, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(ExecutableHarness(executable, HarnessId::Grok)));
        let coordinator = super::HarnessUpdateCoordinator::new(temp.path(), registry);
        let mut watch = coordinator.watch();
        coordinator.start();
        // The startup check is now blocked in the CLI's version probe.
        tokio::time::timeout(Duration::from_secs(5), async {
            while coordinator.status(HarnessId::Grok).phase != HarnessUpdatePhase::Checking
                || !super::lock(&coordinator.inner.operation_gates).contains_key(&HarnessId::Grok)
            {
                watch.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), coordinator.shutdown())
            .await
            .expect("shutdown waited for a probe that can take the full command timeout");
    }

    #[cfg(unix)]
    fn automatic_fixture() -> (tempfile::TempDir, super::HarnessUpdateCoordinator) {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("agent");
        std::fs::write(temp.path().join("version"), "1.0.0\n").unwrap();
        std::fs::write(
            &executable,
            r#"#!/bin/sh
root="$(dirname "$0")"
case "$1:$2" in
  version:) cat "$root/version" ;;
  update:--check)
    test ! -f "$root/fail-check" || exit 1
    printf '2.0.0\n' ;;
  update:) printf '2.0.0\n' > "$root/version" ;;
  *) exit 2 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(ExecutableHarness(executable, HarnessId::Grok)));
        let coordinator = super::HarnessUpdateCoordinator::new(temp.path(), registry);
        (temp, coordinator)
    }

    #[test]
    fn pi_checks_the_active_package_scope() {
        assert!(matches!(
            provider(HarnessId::Pi).latest,
            LatestSource::Npm("@earendil-works/pi-coding-agent")
        ));
    }

    #[test]
    fn opencode_update_check_follows_the_installed_cli_generation() {
        assert_eq!(opencode_release_package("1.18.31"), "opencode-ai");
        assert_eq!(opencode_release_package("2.0.16"), "@opencode/cli");
    }

    #[test]
    fn hermes_checks_report_commit_availability_without_a_version() {
        use super::{UpdateCheck, parse_hermes_update_check};
        assert!(matches!(
            parse_hermes_update_check("→ Fetching from origin...\n✓ Already up to date.\n"),
            Ok(UpdateCheck::Current)
        ));
        for verdict in [
            "⚕ Update available: 1 commit behind origin/main.",
            "☤ Update available: 12 commits behind upstream/main.",
            "⚕ Update available (behind origin/main).",
        ] {
            assert!(matches!(
                parse_hermes_update_check(&format!(
                    "→ Fetching from origin...\n{verdict}\n  Run 'hermes update' to install.\n"
                )),
                Ok(UpdateCheck::Available)
            ));
        }
        for output in [
            "",
            "Hermes 0.20.0",
            "✗ Network error — cannot reach the remote repository.",
        ] {
            assert!(parse_hermes_update_check(output).is_err());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hermes_commit_updates_can_install_without_changing_the_cli_version() {
        use std::os::unix::fs::PermissionsExt;
        use zeron_proto::HarnessUpdatePolicy;

        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("hermes");
        std::fs::write(
            &executable,
            r#"#!/bin/sh
root="$(dirname "$0")"
case "$1:$2" in
  --version:) printf 'Hermes 0.20.0\n' ;;
  update:--check)
    if test -f "$root/updated"; then
      printf '✓ Already up to date.\n'
    else
      printf '☤ Update available: 3 commits behind origin/main.\n'
    fi ;;
  update:--yes) touch "$root/updated" ;;
  *) exit 2 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(ExecutableHarness(executable, HarnessId::Hermes)));
        let coordinator = super::HarnessUpdateCoordinator::new(temp.path(), registry);
        coordinator.check_one(HarnessId::Hermes).await.unwrap();
        let status = coordinator.status(HarnessId::Hermes);
        assert_eq!(status.phase, HarnessUpdatePhase::Available);
        assert_eq!(status.latest_version, None);
        assert!(status.can_apply);

        coordinator.set_policy(HarnessId::Hermes, HarnessUpdatePolicy::AutoWhenIdle);
        wait_for_phase(&coordinator, HarnessId::Hermes, HarnessUpdatePhase::Updated).await;
        assert!(temp.path().join("updated").is_file());
        coordinator.check_one(HarnessId::Hermes).await.unwrap();
        let status = coordinator.status(HarnessId::Hermes);
        assert_eq!(status.phase, HarnessUpdatePhase::Current);
        assert_eq!(status.installed_version.as_deref(), Some("0.20.0"));
        assert_eq!(status.latest_version, None);
        assert!(status.error.is_none());
        coordinator.shutdown().await;
    }

    #[test]
    fn concurrent_publications_never_regress_the_watched_state() {
        let temp = tempfile::tempdir().unwrap();
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(ExecutableHarness(
            temp.path().join("agent"),
            HarnessId::Grok,
        )));
        let coordinator = super::HarnessUpdateCoordinator::new(temp.path(), registry);
        let mut watch = coordinator.watch();
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                let mut previous = 0;
                while !done.load(std::sync::atomic::Ordering::Acquire) {
                    let current = watch.borrow_and_update()[0].checked_at.unwrap_or(0);
                    assert!(
                        current >= previous,
                        "published state regressed: {previous} → {current}"
                    );
                    previous = current;
                    std::thread::yield_now();
                }
            });
            let writers: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        for _ in 0..1000 {
                            coordinator.mutate(HarnessId::Grok, |status| {
                                status.checked_at = Some(status.checked_at.unwrap_or(0) + 1);
                            });
                        }
                    })
                })
                .collect();
            for writer in writers {
                writer.join().unwrap();
            }
            done.store(true, std::sync::atomic::Ordering::Release);
            reader.join().unwrap();
        });
        assert_eq!(coordinator.snapshot()[0].checked_at, Some(4000));
        assert_eq!(*watch.borrow(), coordinator.snapshot());
    }

    async fn wait_for_phase(
        coordinator: &super::HarnessUpdateCoordinator,
        harness: HarnessId,
        phase: HarnessUpdatePhase,
    ) {
        let mut watch = coordinator.watch();
        tokio::time::timeout(Duration::from_secs(5), async {
            while coordinator.status(harness).phase != phase {
                watch.changed().await.unwrap();
            }
        })
        .await
        .unwrap_or_else(|_| panic!("expected {phase:?}, got {:?}", coordinator.status(harness)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn notify_cancels_waiting_automatic_but_preserves_explicit_updates() {
        use zeron_proto::HarnessUpdatePolicy;
        for automatic in [true, false] {
            let (temp, coordinator) = automatic_fixture();
            coordinator.check_one(HarnessId::Grok).await.unwrap();
            let running = coordinator
                .inner
                .registry
                .execution_lease(HarnessId::Grok)
                .await;
            let explicit = if automatic {
                coordinator.set_policy(HarnessId::Grok, HarnessUpdatePolicy::AutoWhenIdle);
                None
            } else {
                // Start explicit work under Auto, without scheduling a
                // competing automatic task as part of fixture setup.
                super::lock(&coordinator.inner.prefs)
                    .policies
                    .insert(HarnessId::Grok, HarnessUpdatePolicy::AutoWhenIdle);
                coordinator.mutate(HarnessId::Grok, |status| {
                    status.policy = HarnessUpdatePolicy::AutoWhenIdle
                });
                let update = coordinator.clone();
                Some(tokio::spawn(
                    async move { update.apply(HarnessId::Grok).await },
                ))
            };
            wait_for_phase(
                &coordinator,
                HarnessId::Grok,
                HarnessUpdatePhase::WaitingForIdle,
            )
            .await;
            coordinator.set_policy(HarnessId::Grok, HarnessUpdatePolicy::Notify);
            if automatic {
                tokio::time::timeout(Duration::from_secs(2), async {
                    while coordinator.inner.registry.update_pending(HarnessId::Grok) {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("Notify cancels without waiting for the active run");
                wait_for_phase(&coordinator, HarnessId::Grok, HarnessUpdatePhase::Available).await;
            } else {
                assert!(
                    !super::lock(&coordinator.inner.cancellations)[&HarnessId::Grok]
                        .cancel
                        .is_cancelled()
                );
            }
            drop(running);
            if let Some(explicit) = explicit {
                explicit.await.unwrap().unwrap();
            }
            // Wait for every task using the operation gate before observing
            // the installed version; no timing-based absence assertion.
            let _operation = coordinator
                .operation_gate(HarnessId::Grok)
                .lock_owned()
                .await;
            assert_eq!(
                std::fs::read_to_string(temp.path().join("version")).unwrap(),
                if automatic { "1.0.0\n" } else { "2.0.0\n" }
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_versionless_available_update_preserves_its_notice() {
        let (temp, _) = automatic_fixture();
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(ExecutableHarness(
            temp.path().join("agent"),
            HarnessId::Hermes,
        )));
        let coordinator = super::HarnessUpdateCoordinator::new(temp.path(), registry.clone());
        coordinator.mutate(HarnessId::Hermes, |status| {
            status.phase = HarnessUpdatePhase::Available;
            status.latest_version = None;
            status.can_apply = true;
        });
        let running = registry.execution_lease(HarnessId::Hermes).await;
        let update = coordinator.clone();
        let apply = tokio::spawn(async move { update.apply(HarnessId::Hermes).await });
        wait_for_phase(
            &coordinator,
            HarnessId::Hermes,
            HarnessUpdatePhase::WaitingForIdle,
        )
        .await;
        assert!(coordinator.cancel(HarnessId::Hermes));
        assert_eq!(apply.await.unwrap().unwrap_err(), "update cancelled");
        let status = coordinator.status(HarnessId::Hermes);
        assert_eq!(status.phase, HarnessUpdatePhase::Available);
        assert_eq!(status.latest_version, None);
        assert!(status.show_update_notice());
        drop(running);
    }

    #[tokio::test]
    async fn automatic_install_boundary_rechecks_policy_for_commands_and_downloads() {
        use zeron_proto::HarnessUpdatePolicy;
        let temp = tempfile::tempdir().unwrap();
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(ExecutableHarness(
            temp.path().join("agent"),
            HarnessId::Grok,
        )));
        let coordinator = super::HarnessUpdateCoordinator::new(temp.path(), registry);
        for phase in [
            HarnessUpdatePhase::Preparing,
            HarnessUpdatePhase::Downloading,
        ] {
            let cancel = tokio_util::sync::CancellationToken::new();
            super::lock(&coordinator.inner.cancellations).insert(
                HarnessId::Grok,
                super::ActiveUpdate {
                    cancel: cancel.clone(),
                    automatic: true,
                    previous_phase: HarnessUpdatePhase::Available,
                },
            );
            coordinator.mutate(HarnessId::Grok, |status| status.phase = phase);
            // Simulate a policy change before the final activation boundary.
            super::lock(&coordinator.inner.prefs)
                .policies
                .insert(HarnessId::Grok, HarnessUpdatePolicy::Notify);
            assert!(coordinator.begin_install(HarnessId::Grok, &cancel).is_err());
            assert!(cancel.is_cancelled());
            assert_eq!(coordinator.status(HarnessId::Grok).phase, phase);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn disabling_an_agent_cancels_its_waiting_automatic_update() {
        use zeron_proto::HarnessUpdatePolicy;
        let (temp, coordinator) = automatic_fixture();
        let registry = &coordinator.inner.registry;
        registry.register(Arc::new(ExecutableHarness(
            temp.path().join("agent"),
            HarnessId::Codex,
        )));
        let running = registry.execution_lease(HarnessId::Grok).await;
        coordinator.mutate(HarnessId::Grok, |status| {
            status.policy = HarnessUpdatePolicy::AutoWhenIdle;
            status.phase = HarnessUpdatePhase::Available;
            status.can_apply = true;
            status.latest_version = Some("2.0.0".into());
        });
        super::lock(&coordinator.inner.prefs)
            .policies
            .insert(HarnessId::Grok, HarnessUpdatePolicy::AutoWhenIdle);
        coordinator.schedule_automatic_update(HarnessId::Grok);
        wait_for_phase(
            &coordinator,
            HarnessId::Grok,
            HarnessUpdatePhase::WaitingForIdle,
        )
        .await;
        registry.set_enabled(HarnessId::Grok, false).unwrap();
        coordinator.refresh_enabled();
        tokio::time::timeout(Duration::from_secs(1), async {
            while registry.update_pending(HarnessId::Grok) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("disable cancels the idle wait before the active run finishes");
        drop(running);
        coordinator.shutdown().await;
        assert_eq!(
            std::fs::read_to_string(temp.path().join("version")).unwrap(),
            "1.0.0\n"
        );
    }

    #[test]
    fn concurrent_preferences_persist_the_latest_complete_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let coordinator =
            super::HarnessUpdateCoordinator::new(temp.path(), Arc::new(HarnessRegistry::new()));
        std::thread::scope(|scope| {
            for worker in 0..8 {
                let coordinator = &coordinator;
                scope.spawn(move || {
                    for revision in 0..100 {
                        super::lock(&coordinator.inner.prefs)
                            .dismissed_versions
                            .insert(HarnessId::Codex, format!("{worker}.{revision}.0"));
                        coordinator.persist_preferences();
                        let bytes = std::fs::read(&coordinator.inner.prefs_path).unwrap();
                        serde_json::from_slice::<super::Preferences>(&bytes)
                            .expect("atomic complete JSON");
                    }
                });
            }
        });
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&coordinator.inner.prefs_path).unwrap()).unwrap();
        assert_eq!(
            saved,
            serde_json::to_value(&*super::lock(&coordinator.inner.prefs)).unwrap()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn enabling_auto_updates_installs_release_discovered_by_its_check() {
        use zeron_proto::HarnessUpdatePolicy;
        for phase in [
            HarnessUpdatePhase::Dormant,
            HarnessUpdatePhase::Checking,
            HarnessUpdatePhase::Current,
        ] {
            let (temp, coordinator) = automatic_fixture();
            coordinator.mutate(HarnessId::Grok, |status| status.phase = phase);
            coordinator.set_policy(HarnessId::Grok, HarnessUpdatePolicy::AutoWhenIdle);
            wait_for_phase(&coordinator, HarnessId::Grok, HarnessUpdatePhase::Updated).await;
            assert_eq!(
                std::fs::read_to_string(temp.path().join("version")).unwrap(),
                "2.0.0\n"
            );
            coordinator.shutdown().await;
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn retrying_one_failed_check_schedules_automatic_installation() {
        use zeron_proto::HarnessUpdatePolicy;
        let (temp, coordinator) = automatic_fixture();
        std::fs::write(temp.path().join("fail-check"), "").unwrap();
        coordinator.set_policy(HarnessId::Grok, HarnessUpdatePolicy::AutoWhenIdle);
        wait_for_phase(&coordinator, HarnessId::Grok, HarnessUpdatePhase::Failed).await;
        std::fs::remove_file(temp.path().join("fail-check")).unwrap();
        coordinator.check_one(HarnessId::Grok).await.unwrap();
        wait_for_phase(&coordinator, HarnessId::Grok, HarnessUpdatePhase::Updated).await;
        assert_eq!(
            std::fs::read_to_string(temp.path().join("version")).unwrap(),
            "2.0.0\n"
        );
        coordinator.shutdown().await;
    }

    #[tokio::test]
    async fn stalled_download_cancels_before_headers_or_mid_body_and_releases_gate() {
        use super::{
            CancellationToken, CodexStageCleanup, HarnessUpdateCoordinator, ReleaseAsset,
            cancellable,
        };
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for send_headers in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let registry = Arc::new(HarnessRegistry::new());
            let coordinator = HarnessUpdateCoordinator::new(temp.path(), registry.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
            let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(socket.read_u8().await.unwrap());
                }
                if send_headers {
                    socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nabc")
                        .await
                        .unwrap();
                }
                let _ = ready_tx.send(());
                let _ = stop_rx.await;
            });
            let archive = temp.path().join("archive.tar.gz");
            let staging = temp.path().join("stage");
            std::fs::create_dir(&staging).unwrap();
            let cancel = CancellationToken::new();
            let download = tokio::spawn({
                let coordinator = coordinator.clone();
                let registry = registry.clone();
                let cancel = cancel.clone();
                let archive = archive.clone();
                let staging = staging.clone();
                async move {
                    let _lease = registry.update_lease(HarnessId::Codex).await;
                    let _cleanup = CodexStageCleanup::new(archive.clone(), staging);
                    let asset = ReleaseAsset {
                        url: format!("http://{address}/release"),
                        digest: String::new(),
                        size: Some(100),
                    };
                    cancellable(
                        &cancel,
                        coordinator.download_codex_archive(
                            HarnessId::Codex,
                            "2.0.0",
                            &asset,
                            &archive,
                        ),
                    )
                    .await
                }
            });
            tokio::time::timeout(Duration::from_secs(5), ready_rx)
                .await
                .unwrap()
                .unwrap();
            if send_headers {
                let mut watch = coordinator.watch();
                tokio::time::timeout(Duration::from_secs(5), async {
                    while coordinator
                        .status(HarnessId::Codex)
                        .progress
                        .as_ref()
                        .and_then(|p| p.completed_bytes)
                        != Some(3)
                    {
                        watch.changed().await.unwrap();
                    }
                })
                .await
                .unwrap();
            }
            cancel.cancel();
            let result = tokio::time::timeout(Duration::from_secs(1), download)
                .await
                .expect("cancellation must not wait for more network data")
                .unwrap();
            assert_eq!(result.unwrap_err(), "update cancelled");
            assert!(!archive.exists());
            assert!(!staging.exists());
            let _run = tokio::time::timeout(
                Duration::from_secs(1),
                registry.execution_lease(HarnessId::Codex),
            )
            .await
            .expect("new runs are unblocked");
            let _ = stop_tx.send(());
            server.await.unwrap();
        }
    }

    #[test]
    fn compares_different_width_versions() {
        assert!(version_is_newer("1.2.1", "1.2"));
        assert!(!version_is_newer("1.2.0", "1.2"));
        assert!(!version_is_newer("1.1.9", "1.2.0"));
    }

    #[test]
    fn codex_is_release_monitored_without_a_guessed_self_update_command() {
        let codex = provider(HarnessId::Codex);
        assert!(matches!(
            codex.latest,
            LatestSource::Github {
                repository: "openai/codex",
                tag_prefix: "rust-v"
            }
        ));
        assert!(codex.update_args.is_none());
        assert!(provider(HarnessId::ClaudeCode).update_args.is_some());
    }

    #[cfg(unix)]
    fn write_codex_release(root: &std::path::Path, version: &str, target: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;

        let release = root.join("releases").join(format!("{version}-{target}"));
        std::fs::create_dir_all(release.join("bin")).unwrap();
        std::fs::create_dir_all(release.join("codex-resources")).unwrap();
        std::fs::create_dir_all(release.join("codex-path")).unwrap();
        let executable = release.join("bin/codex");
        std::fs::write(&executable, "#!/bin/sh\n").unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        std::fs::write(
            release.join("codex-package.json"),
            serde_json::to_vec(&serde_json::json!({
                "layoutVersion": 1,
                "version": version,
                "target": target,
                "variant": "codex",
                "entrypoint": "bin/codex",
                "resourcesDir": "codex-resources",
                "pathDir": "codex-path"
            }))
            .unwrap(),
        )
        .unwrap();
        release
    }

    #[cfg(unix)]
    #[test]
    fn recognizes_and_atomically_activates_official_codex_standalone_layout() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("standalone");
        let first = write_codex_release(&root, "1.0.0", "fixture-target");
        symlink(&first, root.join("current")).unwrap();
        let launcher = temp.path().join("codex");
        symlink(root.join("current/bin/codex"), &launcher).unwrap();

        let install = codex_standalone_install(&launcher).unwrap();
        assert_eq!(install.root, std::fs::canonicalize(&root).unwrap());
        assert_eq!(install.target, "fixture-target");

        let next = write_codex_release(&root, "2.0.0", "fixture-target");
        validate_codex_package(&next, "2.0.0", "fixture-target").unwrap();
        activate_codex_release(&root, &next, uuid::Uuid::new_v4()).unwrap();
        assert_eq!(
            std::fs::canonicalize(root.join("current")).unwrap(),
            std::fs::canonicalize(next).unwrap()
        );
        assert_eq!(
            std::fs::read_link(&launcher).unwrap(),
            root.join("current/bin/codex")
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_codex_package_with_mismatched_identity() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("standalone");
        let release = write_codex_release(&root, "2.0.0", "fixture-target");
        assert!(validate_codex_package(&release, "2.0.1", "fixture-target").is_err());
        assert!(validate_codex_package(&release, "2.0.0", "other-target").is_err());
    }

    #[tokio::test]
    async fn accepted_cancellation_and_installation_are_mutually_exclusive() {
        let temp = tempfile::tempdir().unwrap();
        let registry = Arc::new(HarnessRegistry::new());
        let harness = HarnessId::ClaudeCode;
        registry.register(Arc::new(ExecutableHarness(
            temp.path().join("agent"),
            harness,
        )));
        let coordinator = super::HarnessUpdateCoordinator::new(temp.path(), registry);

        // Preparing covers vendor commands; Downloading covers staged Codex
        // activation. Exercise both orderings and simultaneous contenders.
        for phase in [
            HarnessUpdatePhase::Preparing,
            HarnessUpdatePhase::Downloading,
        ] {
            for ordering in 0..66 {
                let token = tokio_util::sync::CancellationToken::new();
                super::lock(&coordinator.inner.cancellations).insert(
                    harness,
                    super::ActiveUpdate {
                        cancel: token.clone(),
                        automatic: false,
                        previous_phase: HarnessUpdatePhase::Available,
                    },
                );
                coordinator.mutate(harness, |status| status.phase = phase);
                let barrier = std::sync::Barrier::new(2);
                let (cancelled, installed) = match ordering {
                    0 => {
                        let cancelled = coordinator.cancel(harness);
                        (
                            cancelled,
                            coordinator.begin_install(harness, &token).is_ok(),
                        )
                    }
                    1 => {
                        let installed = coordinator.begin_install(harness, &token).is_ok();
                        (coordinator.cancel(harness), installed)
                    }
                    _ => std::thread::scope(|scope| {
                        let barrier = &barrier;
                        let coordinator = &coordinator;
                        let cancellation = scope.spawn(move || {
                            barrier.wait();
                            coordinator.cancel(harness)
                        });
                        barrier.wait();
                        let installed = coordinator.begin_install(harness, &token).is_ok();
                        (cancellation.join().unwrap(), installed)
                    }),
                };
                assert_ne!(cancelled, installed);
                assert_eq!(token.is_cancelled(), cancelled);
                assert_eq!(
                    coordinator.status(harness).phase,
                    if installed {
                        HarnessUpdatePhase::Installing
                    } else {
                        phase
                    }
                );
            }
        }
    }

    #[tokio::test]
    async fn a_check_refresh_does_not_overwrite_an_installing_phase() {
        let temp = tempfile::tempdir().unwrap();
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(ExecutableHarness(
            temp.path().join("agent"),
            HarnessId::ClaudeCode,
        )));
        let coordinator = super::HarnessUpdateCoordinator::new(temp.path(), registry);
        super::lock(&coordinator.inner.cancellations).insert(
            HarnessId::ClaudeCode,
            super::ActiveUpdate {
                cancel: tokio_util::sync::CancellationToken::new(),
                automatic: false,
                previous_phase: HarnessUpdatePhase::Available,
            },
        );
        coordinator.mutate(HarnessId::ClaudeCode, |status| {
            status.phase = HarnessUpdatePhase::Installing
        });

        coordinator.check_one(HarnessId::ClaudeCode).await.unwrap();

        assert_eq!(
            coordinator.status(HarnessId::ClaudeCode).phase,
            HarnessUpdatePhase::Installing
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_a_busy_host_does_not_touch_another_device_or_its_installation() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(ExecutableHarness(
            first.path().join("agent"),
            HarnessId::ClaudeCode,
        )));
        let other_registry = Arc::new(HarnessRegistry::new());
        other_registry.register(Arc::new(ExecutableHarness(
            second.path().join("agent"),
            HarnessId::ClaudeCode,
        )));
        let host = super::HarnessUpdateCoordinator::new(first.path(), registry.clone());
        let other = super::HarnessUpdateCoordinator::new(second.path(), other_registry);
        for coordinator in [&host, &other] {
            coordinator.mutate(HarnessId::ClaudeCode, |status| {
                status.phase = HarnessUpdatePhase::Available;
                status.installed_version = Some("1.0.0".into());
                status.latest_version = Some("2.0.0".into());
            });
        }
        let run = registry.execution_lease(HarnessId::ClaudeCode).await;
        let applying = tokio::spawn({
            let host = host.clone();
            async move { host.apply(HarnessId::ClaudeCode).await }
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while host.status(HarnessId::ClaudeCode).phase != HarnessUpdatePhase::WaitingForIdle {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            other.status(HarnessId::ClaudeCode).phase,
            HarnessUpdatePhase::Available
        );
        assert!(host.cancel(HarnessId::ClaudeCode));
        let result = tokio::time::timeout(Duration::from_secs(2), applying)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err(), "update cancelled");
        assert!(!registry.update_pending(HarnessId::ClaudeCode));
        assert_eq!(
            other
                .status(HarnessId::ClaudeCode)
                .installed_version
                .as_deref(),
            Some("1.0.0")
        );
        drop(run);
        // A host restart rebuilds live state and probes again; it never replays
        // an old update request or restores an invented Installing snapshot.
        let restarted = super::HarnessUpdateCoordinator::new(first.path(), registry);
        assert_eq!(
            restarted.status(HarnessId::ClaudeCode).phase,
            HarnessUpdatePhase::Checking
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn installing_survives_the_request_task_being_dropped_and_verifies() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("fixture-agent");
        std::fs::write(temp.path().join("version"), "1.0.0\n").unwrap();
        std::fs::write(
            &executable,
            r#"#!/bin/sh
version_file="$(dirname "$0")/version"
case "$1" in
  --version) cat "$version_file" ;;
  update) sleep 0.2; printf '2.0.0\n' > "$version_file" ;;
  *) exit 2 ;;
esac
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();

        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(ExecutableHarness(
            executable,
            HarnessId::ClaudeCode,
        )));
        let coordinator = super::HarnessUpdateCoordinator::new(temp.path(), registry.clone());
        coordinator.mutate(HarnessId::ClaudeCode, |status| {
            status.installed_version = Some("1.0.0".into());
            status.latest_version = Some("2.0.0".into());
            status.phase = HarnessUpdatePhase::Available;
        });

        let apply = tokio::spawn({
            let coordinator = coordinator.clone();
            async move { coordinator.apply(HarnessId::ClaudeCode).await }
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while coordinator.status(HarnessId::ClaudeCode).phase != HarnessUpdatePhase::Installing
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        apply.abort();

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let status = coordinator.status(HarnessId::ClaudeCode);
                if matches!(
                    status.phase,
                    HarnessUpdatePhase::Updated | HarnessUpdatePhase::Current
                ) {
                    assert_eq!(status.installed_version.as_deref(), Some("2.0.0"));
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!registry.update_pending(HarnessId::ClaudeCode));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_cancels_apply_queued_behind_provider_check() {
        let (temp, coordinator) = automatic_fixture();
        coordinator.check_one(HarnessId::Grok).await.unwrap();
        let checking = coordinator
            .operation_gate(HarnessId::Grok)
            .lock_owned()
            .await;
        let applying = tokio::spawn({
            let coordinator = coordinator.clone();
            async move { coordinator.apply(HarnessId::Grok).await }
        });
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        coordinator.shutdown().await;
        // Queued work must settle without needing the old checker to finish.
        let result = tokio::time::timeout(Duration::from_secs(1), applying)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err(), "update cancelled");
        drop(checking);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("version")).unwrap(),
            "1.0.0\n"
        );
        assert!(!coordinator.inner.registry.update_pending(HarnessId::Grok));
        assert_eq!(
            coordinator.apply(HarnessId::Grok).await.unwrap_err(),
            "update cancelled"
        );
    }

    #[test]
    fn claude_channel_comes_from_vendor_diagnostics() {
        for channel in ["stable", "latest"] {
            let output = format!(
                "Claude Code doctor\nAuto-update channel: {channel}\nNo installation issues found.\n"
            );
            assert_eq!(super::parse_claude_release_channel(&output), Some(channel));
        }
        for output in ["", "Claude 2.1.100", "Auto-update channel: unknown"] {
            assert_eq!(super::parse_claude_release_channel(output), None);
        }
    }

    #[test]
    fn package_managed_claude_has_manual_guidance_and_cask_channel() {
        for (path, command, channel) in [
            (
                "/opt/homebrew/Caskroom/claude-code/2.1.100/claude",
                "brew upgrade claude-code",
                "stable",
            ),
            (
                "/opt/homebrew/Caskroom/claude-code@latest/2.1.110/claude",
                "brew upgrade claude-code@latest",
                "latest",
            ),
        ] {
            let path = std::path::Path::new(path);
            assert!(!super::can_apply_update(HarnessId::ClaudeCode, path));
            assert_eq!(
                super::claude_package_manager_command(path).as_deref(),
                Some(command)
            );
            assert_eq!(super::claude_cask_channel(path).unwrap(), channel);
        }
        assert!(!super::can_apply_update(
            HarnessId::ClaudeCode,
            std::path::Path::new("/usr/bin/claude")
        ));
        assert!(super::can_apply_update(
            HarnessId::ClaudeCode,
            std::path::Path::new("/home/test/.local/bin/claude")
        ));
        let npm =
            std::path::Path::new("/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js");
        assert!(super::can_apply_update(HarnessId::ClaudeCode, npm));
        assert_eq!(super::claude_cask_channel(npm), None);
        assert_eq!(super::claude_package_manager_command(npm), None);
        assert!(super::can_apply_update(
            HarnessId::Pi,
            std::path::Path::new("/usr/bin/pi")
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unknown_claude_channel_clears_stale_release_and_never_auto_installs() {
        use std::os::unix::fs::PermissionsExt;
        use zeron_proto::HarnessUpdatePolicy;
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("claude");
        std::fs::write(&executable, "#!/bin/sh\ncase $1 in\n --version) echo '2.1.100 (Claude Code)' ;;\n doctor) echo 'Old diagnostic with no channel' ;;\n update) exit 42 ;;\nesac\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(ExecutableHarness(
            executable,
            HarnessId::ClaudeCode,
        )));
        let coordinator = super::HarnessUpdateCoordinator::new(temp.path(), registry);
        super::lock(&coordinator.inner.prefs)
            .policies
            .insert(HarnessId::ClaudeCode, HarnessUpdatePolicy::AutoWhenIdle);
        coordinator.mutate(HarnessId::ClaudeCode, |status| {
            status.phase = HarnessUpdatePhase::Available;
            status.latest_version = Some("2.1.110".into());
            status.channel = Some("latest".into());
        });
        coordinator.check_one(HarnessId::ClaudeCode).await.unwrap();
        tokio::task::yield_now().await;
        let status = coordinator.status(HarnessId::ClaudeCode);
        assert_eq!(status.phase, HarnessUpdatePhase::ManualActionRequired);
        assert_eq!(status.channel, None);
        assert_eq!(status.latest_version, None);
        assert!(status.can_apply, "explicit vendor update remains available");
        assert!(!coordinator.automatic_update_ready(HarnessId::ClaudeCode));
        assert_eq!(status.manual_command.as_deref(), Some("claude update"));
        assert!(
            !coordinator
                .inner
                .registry
                .update_pending(HarnessId::ClaudeCode)
        );
        coordinator.shutdown().await;
    }
}
