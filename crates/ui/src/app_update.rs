//! Desktop self-update: the app's own release checker plus the download /
//! install lifecycle behind the sidebar update strip, "Check for Updates…"
//! (macOS app menu; the account menu elsewhere) and install-on-quit.
//!
//! The checker runs in this process against this binary's version. The
//! engine keeps its own checker for daemons and remote reporting, but the UI
//! may be attached to a daemon of another version, whose status describes
//! that binary rather than the app the user is looking at.
//!
//! Lifecycle, idiomatic to desktop updaters (Sparkle, VS Code, Zed): a found
//! release downloads in the background and is verified, the strip offers
//! "restart to apply", and a staged update the user never restarts for is
//! installed when the app quits — so nobody stays on a stale version just
//! because they never clicked. `ZERON_AUTO_UPDATE=0` keeps it report-only.

use std::path::{Path, PathBuf};

use gpui::{App, AppContext as _, Context, Entity, Global, SharedString, Task};
use gpui_tokio::Tokio;
use zeron_update::{InstallKind, UpdateBlocker, UpdateStatus, Updater};

/// Download/install lifecycle of the newest release in this process.
#[derive(Debug, Clone, PartialEq)]
pub enum Flow {
    Idle,
    Downloading {
        version: String,
    },
    /// Staged and verified: "restart to apply", or installed on quit.
    Ready {
        version: String,
        staged: PathBuf,
    },
    Failed {
        version: String,
        message: SharedString,
    },
    /// Swapped in; this process is on its way out.
    Installed,
}

/// The user-initiated check's dialog.
#[derive(Debug, Clone, PartialEq)]
pub enum Prompt {
    Checking,
    /// The check finished; the dialog renders from the live status + flow.
    Result,
    CheckFailed(SharedString),
}

/// What clicking the sidebar strip does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripAction {
    /// Nothing — progress only.
    None,
    Download,
    Restart,
    /// Explain why this install can't update itself (dialog).
    Explain,
    /// Advisory installs: open the releases page (unmanaged) and dismiss.
    Advise {
        open_releases: bool,
    },
}

pub struct AppUpdate {
    install: InstallKind,
    blocker: Option<UpdateBlocker>,
    /// Background download + install on quit (`ZERON_AUTO_UPDATE` unset or on).
    automatic: bool,
    edge_url: String,
    data_dir: PathBuf,
    checker: Updater,
    status: Option<UpdateStatus>,
    flow: Flow,
    prompt: Option<Prompt>,
    /// Version whose advisory strip the user dismissed (a newer release shows
    /// it again).
    dismissed: Option<String>,
    _status_watch: Task<()>,
    download: Option<Task<()>>,
    user_check: Option<Task<()>>,
}

struct GlobalAppUpdate(Entity<AppUpdate>);

impl Global for GlobalAppUpdate {}

impl AppUpdate {
    /// Start the desktop checker. Call once at boot, after `gpui_tokio` is
    /// initialized.
    pub fn init(edge_url: String, data_dir: PathBuf, cx: &mut App) {
        let entity = cx.new(|cx| Self::new(edge_url, data_dir, cx));
        cx.set_global(GlobalAppUpdate(entity));
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalAppUpdate>()
            .map(|global| global.0.clone())
    }

    fn new(edge_url: String, data_dir: PathBuf, cx: &mut Context<Self>) -> Self {
        let install = zeron_update::detect_install();
        let blocker = install
            .supports_desktop_update()
            .then(|| install.desktop_update_blocker())
            .flatten();
        let checker = {
            let _runtime = Tokio::handle(cx).enter();
            Updater::spawn_desktop(edge_url.clone())
        };
        let mut statuses = checker.watch();
        let status_watch = cx.spawn(async move |this, cx| {
            while statuses.changed().await.is_ok() {
                let status = statuses.borrow_and_update().clone();
                if this
                    .update(cx, |this, cx| this.apply_status(status, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        tracing::info!(?install, ?blocker, "desktop update checker started");
        Self {
            install,
            blocker,
            automatic: zeron_update::desktop_auto_update_enabled(),
            edge_url,
            data_dir,
            checker,
            status: None,
            flow: Flow::Idle,
            prompt: None,
            dismissed: None,
            _status_watch: status_watch,
            download: None,
            user_check: None,
        }
    }

    pub fn install(&self) -> &InstallKind {
        &self.install
    }

    pub fn blocker(&self) -> Option<&UpdateBlocker> {
        self.blocker.as_ref()
    }

    pub fn flow(&self) -> &Flow {
        &self.flow
    }

    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
    }

    /// The newer release the last check found, if any.
    pub fn available(&self) -> Option<&str> {
        self.status
            .as_ref()
            .filter(|status| status.update_available)
            .and_then(|status| status.latest_version.as_deref())
    }

    /// Whether this app downloads and installs updates itself right now.
    pub fn self_updating(&self) -> bool {
        self.install.supports_desktop_update() && self.blocker.is_none()
    }

    /// Window activation / wake: check if one is due by the wall clock.
    pub fn poke(&self) {
        self.checker.poke();
    }

    fn apply_status(&mut self, status: UpdateStatus, cx: &mut Context<Self>) {
        let succeeded = status.error.is_none();
        self.status = Some(status);
        if succeeded && self.automatic {
            self.download_if_needed(cx);
        }
        cx.notify();
    }

    /// Start (or restart) the background download unless the newest release
    /// is already downloading or staged. A failed download retries at the
    /// next successful check.
    fn download_if_needed(&mut self, cx: &mut Context<Self>) {
        if !self.self_updating() {
            return;
        }
        let Some(latest) = self.available() else {
            return;
        };
        let current = match &self.flow {
            Flow::Downloading { .. } | Flow::Installed => return,
            Flow::Ready { version, .. } => Some(version),
            Flow::Idle | Flow::Failed { .. } => None,
        };
        if current.is_some_and(|version| !zeron_update::version_newer(latest, version)) {
            return;
        }
        self.start_download(cx);
    }

    pub fn start_download(&mut self, cx: &mut Context<Self>) {
        if !self.self_updating() || matches!(self.flow, Flow::Downloading { .. }) {
            return;
        }
        let Some(version) = self.available().map(str::to_owned) else {
            return;
        };
        let edge_url = self.edge_url.clone();
        let data_dir = self.data_dir.clone();
        let install = self.install.clone();
        self.flow = Flow::Downloading {
            version: version.clone(),
        };
        let stage = Tokio::spawn(cx, async move {
            // Re-read the manifest so a long-lived status still downloads the
            // newest release, with that release's checksums.
            let manifest = zeron_update::fetch_latest(&edge_url).await?;
            anyhow::ensure!(
                zeron_update::version_newer(&manifest.version, zeron_update::current_version()),
                "the release feed no longer offers a newer version"
            );
            let staged = install
                .stage_desktop(&edge_url, &manifest, &data_dir)
                .await?;
            anyhow::Ok((manifest.version, staged))
        });
        self.download = Some(cx.spawn(async move |this, cx| {
            let outcome = match stage.await {
                Ok(Ok(staged)) => Ok(staged),
                Ok(Err(err)) => Err(format!("{err:#}")),
                Err(join) => Err(join.to_string()),
            };
            this.update(cx, |this, cx| {
                this.flow = match outcome {
                    Ok((version, staged)) => {
                        tracing::info!(%version, staged = %staged.display(), "update staged");
                        Flow::Ready { version, staged }
                    }
                    Err(message) => {
                        tracing::warn!(%message, "update download failed");
                        Flow::Failed {
                            version,
                            message: message.into(),
                        }
                    }
                };
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// "Check for Updates…": check now and report the outcome in a dialog. A
    /// found release starts downloading at once (it would anyway on the
    /// hourly check), and a failed download gets a fresh attempt.
    pub fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        self.prompt = Some(Prompt::Checking);
        let checker = self.checker.clone();
        let check = Tokio::spawn(cx, async move { checker.check().await });
        self.user_check = Some(cx.spawn(async move |this, cx| {
            let result = match check.await {
                Ok(Ok(status)) => Ok(status),
                Ok(Err(err)) => {
                    tracing::warn!(error = %format!("{err:#}"), "update check failed");
                    // The dialog shows the cause; the log keeps the chain.
                    Err(err.root_cause().to_string())
                }
                Err(join) => Err(join.to_string()),
            };
            this.update(cx, |this, cx| {
                let outcome = match result {
                    Ok(status) => {
                        this.status = Some(status);
                        if this.automatic {
                            this.download_if_needed(cx);
                        }
                        Prompt::Result
                    }
                    Err(message) => Prompt::CheckFailed(message.into()),
                };
                // The user may have closed the dialog while it was checking.
                if this.prompt.is_some() {
                    this.prompt = Some(outcome);
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub fn dismiss_prompt(&mut self, cx: &mut Context<Self>) {
        self.prompt = None;
        self.user_check = None;
        cx.notify();
    }

    /// Open the dialog on its current result (the strip's "explain" action).
    pub fn show_result(&mut self, cx: &mut Context<Self>) {
        self.prompt = Some(Prompt::Result);
        cx.notify();
    }

    /// The staged update "Restart to update" would install.
    pub fn staged(&self) -> Option<PathBuf> {
        match &self.flow {
            Flow::Ready { staged, .. } => Some(staged.clone()),
            _ => None,
        }
    }

    /// Install `staged` and arrange the relaunch. The caller quits on success.
    pub fn install_for_restart(
        &mut self,
        staged: &Path,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let result = self.install.apply_desktop(staged, true);
        self.flow = match &result {
            Ok(()) => Flow::Installed,
            Err(err) => {
                tracing::error!(error = %err, "update apply failed");
                Flow::Failed {
                    version: self.available().unwrap_or_default().to_owned(),
                    message: format!("{err:#}").into(),
                }
            }
        };
        cx.notify();
        result
    }

    /// Quit hook: a staged update the user never restarted for installs now,
    /// so the next launch is current. Runs synchronously inside the quit.
    fn install_on_quit(&mut self) {
        if !self.automatic {
            return;
        }
        let Flow::Ready { version, staged } = &self.flow else {
            return;
        };
        match self.install.apply_desktop(staged, false) {
            Ok(()) => {
                tracing::info!(%version, "installed staged update on quit");
                self.flow = Flow::Installed;
            }
            Err(err) => tracing::warn!(error = %err, "installing the staged update on quit failed"),
        }
    }

    pub fn dismiss_advisory(&mut self, cx: &mut Context<Self>) {
        self.dismissed = self.available().map(str::to_owned);
        cx.notify();
    }

    /// The sidebar strip for the current state: `None` while there is
    /// nothing newer (or the advisory was dismissed for this version).
    pub fn strip(&self) -> Option<(SharedString, StripAction)> {
        let latest = self.available()?;
        if self.dismissed.as_deref() == Some(latest) {
            return None;
        }
        Some(strip_for(
            &self.install,
            self.blocker.is_some(),
            &self.flow,
            latest,
        ))
    }
}

/// Label + click action of the strip. Self-updating installs drive their flow
/// from it; blocked installs explain themselves; managed installs without a
/// desktop path get the `zeron update` hint; unmanaged installs (source builds,
/// hand-copied binaries) are pointed at the GitHub releases page.
pub fn strip_for(
    install: &InstallKind,
    blocked: bool,
    flow: &Flow,
    latest: &str,
) -> (SharedString, StripAction) {
    if install.supports_desktop_update() {
        if blocked {
            return (
                format!("Update available — v{latest}").into(),
                StripAction::Explain,
            );
        }
        return match flow {
            Flow::Idle => (
                format!("Update available — v{latest}").into(),
                StripAction::Download,
            ),
            Flow::Downloading { version } => {
                (format!("Downloading v{version}…").into(), StripAction::None)
            }
            Flow::Ready { .. } => (
                "Update ready — restart to apply".into(),
                StripAction::Restart,
            ),
            Flow::Failed { message, .. } => (
                format!("Update failed: {message}").into(),
                StripAction::Download,
            ),
            Flow::Installed => ("Restarting…".into(), StripAction::None),
        };
    }
    if matches!(install, InstallKind::Managed { .. }) {
        (
            format!("Update available — v{latest} · run `zeron update`").into(),
            StripAction::Advise {
                open_releases: false,
            },
        )
    } else {
        (
            format!("Update available — v{latest} · download from GitHub").into(),
            StripAction::Advise {
                open_releases: true,
            },
        )
    }
}

/// "Check for Updates…" from a menu: surface the most recent window, then check.
pub fn check_for_updates(cx: &mut App) {
    crate::window_manager::activate(cx);
    if let Some(update) = AppUpdate::global(cx) {
        update.update(cx, |update, cx| update.check_for_updates(cx));
    }
}

/// See [`AppUpdate::install_on_quit`].
pub fn install_on_quit(cx: &mut App) {
    if let Some(update) = AppUpdate::global(cx) {
        update.update(cx, |update, _| update.install_on_quit());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_follows_the_flow_on_self_updating_installs() {
        let mac_app = InstallKind::MacApp {
            bundle: PathBuf::from("/Applications/Zeron.app"),
        };
        assert_eq!(
            strip_for(&mac_app, false, &Flow::Idle, "0.2.86"),
            (
                SharedString::from("Update available — v0.2.86"),
                StripAction::Download
            )
        );
        let downloading = Flow::Downloading {
            version: "0.2.86".into(),
        };
        assert_eq!(
            strip_for(&mac_app, false, &downloading, "0.2.86"),
            (
                SharedString::from("Downloading v0.2.86…"),
                StripAction::None
            )
        );
        let ready = Flow::Ready {
            version: "0.2.86".into(),
            staged: PathBuf::from("/tmp/Zeron.app"),
        };
        assert_eq!(
            strip_for(&mac_app, false, &ready, "0.2.86").1,
            StripAction::Restart
        );
        let failed = Flow::Failed {
            version: "0.2.86".into(),
            message: "offline".into(),
        };
        assert_eq!(
            strip_for(&mac_app, false, &failed, "0.2.86"),
            (
                SharedString::from("Update failed: offline"),
                StripAction::Download
            )
        );
        // A translocated or read-only bundle explains instead of failing.
        assert_eq!(
            strip_for(&mac_app, true, &Flow::Idle, "0.2.86").1,
            StripAction::Explain
        );
    }

    #[test]
    fn strip_advises_installs_without_a_desktop_path() {
        let unmanaged = strip_for(&InstallKind::Unmanaged, false, &Flow::Idle, "0.2.86");
        assert_eq!(
            unmanaged,
            (
                SharedString::from("Update available — v0.2.86 · download from GitHub"),
                StripAction::Advise {
                    open_releases: true
                }
            )
        );
        let managed = InstallKind::Managed {
            app_root: PathBuf::from("/home/u/.zeron/app"),
        };
        let strip = strip_for(&managed, false, &Flow::Idle, "0.2.86");
        if cfg!(target_os = "linux") {
            // Linux desktop installs share the managed layout and update
            // themselves like the other desktop platforms.
            assert_eq!(strip.1, StripAction::Download);
        } else {
            assert_eq!(
                strip.0,
                SharedString::from("Update available — v0.2.86 · run `zeron update`")
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_installs_drive_the_desktop_flow() {
        let installed = InstallKind::WindowsPortable {
            directory: PathBuf::from(r"C:\Users\u\AppData\Local\Programs\Zeron"),
        };
        assert_eq!(
            strip_for(&installed, false, &Flow::Idle, "0.2.86"),
            (
                SharedString::from("Update available — v0.2.86"),
                StripAction::Download
            )
        );
    }
}
