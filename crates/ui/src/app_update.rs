//! Desktop self-update: the app's own release checker plus the download /
//! install lifecycle behind the sidebar update strip, "Check for Updates…"
//! (macOS app menu; the account menu elsewhere) and install-on-quit.
//!
//! The checker runs in this process against this binary's version. The
//! engine keeps its own checker for daemons and remote reporting, but the UI
//! may be attached to a daemon of another version, whose status describes
//! that binary rather than the app the user is looking at.
//!
//! Lifecycle: a found release downloads in the background and is verified,
//! then installs WITHOUT asking. On Unix the window is replaced by the new
//! build at the next idle or unfocused moment ([`may_swap_now`]) while the
//! engine host it is attached to keeps running, so agents and terminals never
//! notice (see `docs/live-update.md`, "UI swap"). The strip only reports
//! "Updated to vX". Windows has no engine host to keep alive, so a staged
//! update installs when the app quits, without a prompt. `ZERON_AUTO_UPDATE=0`
//! or the "Automatic updates" setting keeps it report-only ("restart to
//! apply", as before).

use std::path::{Path, PathBuf};
use std::time::Duration;

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
    /// Installed on disk; only the window still has to be replaced. Kept (and
    /// retried, without downloading or installing again) when the new window
    /// does not come up.
    Applied {
        version: String,
    },
    /// This window is the new build a silent swap started: an informational
    /// "Updated to vX" until dismissed.
    Swapped {
        version: String,
    },
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
    /// The informational "Updated to vX" strip: dismiss it.
    DismissUpdated,
}

/// What decides whether the window may be replaced right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapGate {
    pub window_active: bool,
    /// Time since the last key press, click or scroll in the window.
    pub idle_for: Duration,
    /// An IME composition is in progress.
    pub composing: bool,
    /// A file surface holds edits that are not saved.
    pub unsaved_edits: bool,
}

/// After a swap that did not happen, wait this long before trying again.
const SWAP_RETRY_AFTER: Duration = Duration::from_secs(10 * 60);

/// How long an active window must sit untouched before it may be replaced.
pub const SWAP_IDLE: Duration = Duration::from_secs(60);

/// Replace the window only when nobody is mid-thought: never during an IME
/// composition or with unsaved file edits, and otherwise only when the window
/// is in the background or has been idle for [`SWAP_IDLE`].
pub fn may_swap_now(gate: &SwapGate) -> bool {
    !gate.composing && !gate.unsaved_edits && (!gate.window_active || gate.idle_for >= SWAP_IDLE)
}

/// The persisted choices [`next_action`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub auto_update: bool,
}

/// What to do about a staged update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing is staged.
    Nothing,
    /// Report only: the strip offers "restart to apply".
    ShowStrip,
    /// Install and swap the window now.
    Install,
    /// Wanted, but the user is busy: ask again shortly.
    Wait,
    /// No engine host to keep alive (Windows): install when the app quits.
    InstallOnQuit,
}

/// [`next_action_for_os`] for this platform and environment.
pub fn next_action(flow: &Flow, settings: Settings, gate: SwapGate) -> Action {
    next_action_for_os(std::env::consts::OS, flow, settings, gate)
}

pub fn next_action_for_os(os: &str, flow: &Flow, settings: Settings, gate: SwapGate) -> Action {
    decide(
        os,
        flow,
        settings,
        gate,
        zeron_update::desktop_auto_update_enabled(),
    )
}

/// [`next_action_for_os`] with `ZERON_AUTO_UPDATE` already read (`env_enabled`).
fn decide(os: &str, flow: &Flow, settings: Settings, gate: SwapGate, env_enabled: bool) -> Action {
    if !matches!(flow, Flow::Ready { .. } | Flow::Applied { .. }) {
        return Action::Nothing;
    }
    if !env_enabled || !settings.auto_update {
        return Action::ShowStrip;
    }
    if os == "windows" {
        return Action::InstallOnQuit;
    }
    if may_swap_now(&gate) {
        Action::Install
    } else {
        Action::Wait
    }
}

/// The informational strip after a swap (and for the states that have no
/// action of their own).
pub fn strip_text(flow: &Flow) -> Option<String> {
    match flow {
        Flow::Swapped { version } => Some(format!("Updated to v{version}")),
        _ => None,
    }
}

pub struct AppUpdate {
    install: InstallKind,
    blocker: Option<UpdateBlocker>,
    /// Background download + install on quit (`ZERON_AUTO_UPDATE` unset or on).
    automatic: bool,
    /// The "Automatic updates" setting: install and swap without asking.
    auto_update: bool,
    /// The swap failed recently: leave the user alone until this passes.
    swap_retry_at: Option<std::time::Instant>,
    /// The engine this window is attached to keeps running when the window
    /// goes away (an engine host or a daemon). An engine embedded in this
    /// window dies with it, so a window swap would kill every run and PTY.
    engine_survives: bool,
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
            auto_update: true,
            swap_retry_at: None,
            engine_survives: false,
            edge_url,
            data_dir: data_dir.clone(),
            checker,
            status: None,
            flow: take_swap_marker(&data_dir, zeron_update::current_version())
                .map_or(Flow::Idle, |version| Flow::Swapped { version }),
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
            Flow::Downloading { .. } | Flow::Installed | Flow::Applied { .. } => return,
            Flow::Ready { version, .. } => Some(version),
            Flow::Idle | Flow::Failed { .. } | Flow::Swapped { .. } => None,
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
        if !self.automatic || !self.auto_update {
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

    /// The informational "Updated to vX" strip goes away.
    pub fn dismiss_updated(&mut self, cx: &mut Context<Self>) {
        if matches!(self.flow, Flow::Swapped { .. }) {
            self.flow = Flow::Idle;
            cx.notify();
        }
    }

    /// The "Automatic updates" setting (from the persisted UI settings).
    pub fn set_auto_update(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.auto_update != on {
            self.auto_update = on;
            cx.notify();
        }
    }

    pub fn auto_update(&self) -> bool {
        self.auto_update
    }

    /// Whether the engine outlives this window (see the field).
    pub fn set_engine_survives(&mut self, survives: bool, cx: &mut Context<Self>) {
        if self.engine_survives != survives {
            self.engine_survives = survives;
            cx.notify();
        }
    }

    /// Whether a staged update installs without asking (as opposed to
    /// "restart to apply").
    pub fn silent(&self) -> bool {
        // Unix installs swap the window while the engine host keeps running;
        // with an engine embedded in the window that would kill every run, so
        // it stays "restart to apply" (Windows installs on quit either way).
        self.automatic
            && self.auto_update
            && self.self_updating()
            && (cfg!(windows) || self.engine_survives)
    }

    /// What the shell should do about the staged update given how busy the
    /// user is; see [`next_action`].
    pub fn swap_action(&self, gate: SwapGate) -> Action {
        if !self.self_updating() {
            return Action::Nothing;
        }
        if !cfg!(windows) && !self.engine_survives {
            return match self.flow {
                Flow::Ready { .. } | Flow::Applied { .. } => Action::ShowStrip,
                _ => Action::Nothing,
            };
        }
        if self
            .swap_retry_at
            .is_some_and(|at| std::time::Instant::now() < at)
        {
            return Action::Wait;
        }
        next_action(
            &self.flow,
            Settings {
                auto_update: self.auto_update,
            },
            gate,
        )
    }

    /// Install the staged update for a swap (or, when it is already installed
    /// from an earlier attempt, just say so): nothing is relaunched or quit
    /// here (the shell starts the new window and closes this one). Returns the
    /// installed version.
    pub fn install_for_swap(&mut self, cx: &mut Context<Self>) -> anyhow::Result<String> {
        let (version, staged) = match self.flow.clone() {
            Flow::Applied { version } => return Ok(version),
            Flow::Ready { version, staged } => (version, staged),
            _ => anyhow::bail!("no update is staged"),
        };
        let result = self
            .install
            .apply_desktop(&staged, false)
            .and_then(|()| write_swap_marker(&self.data_dir, &version));
        match &result {
            Ok(()) => {
                tracing::info!(%version, "update installed; swapping the window");
                self.flow = Flow::Applied {
                    version: version.clone(),
                };
            }
            Err(err) => {
                tracing::error!(error = %err, "installing the update for a swap failed");
                self.flow = Flow::Failed {
                    version: version.clone(),
                    message: format!("{err:#}").into(),
                };
            }
        }
        cx.notify();
        result.map(|()| version)
    }

    /// The new window did not come up (or the user got busy again): the update
    /// stays installed and the swap is retried later, without installing again.
    pub fn swap_postponed(&mut self, message: &str, cx: &mut Context<Self>) {
        tracing::warn!(%message, "the window swap did not happen; will retry later");
        self.swap_retry_at = Some(std::time::Instant::now() + SWAP_RETRY_AFTER);
        cx.notify();
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The sidebar strip for the current state: `None` while there is
    /// nothing newer (or the advisory was dismissed for this version).
    pub fn strip(&self) -> Option<(SharedString, StripAction)> {
        if let Some(text) = strip_text(&self.flow) {
            return Some((text.into(), StripAction::DismissUpdated));
        }
        // A silent install has nothing to ask: the staged update goes in when
        // the user is away (clicking installs it now).
        if self.silent() && matches!(self.flow, Flow::Ready { .. }) {
            return Some(if cfg!(windows) {
                (
                    "Update ready — installs when you quit".into(),
                    StripAction::None,
                )
            } else {
                (
                    "Update ready — installing when you're away".into(),
                    StripAction::Restart,
                )
            });
        }
        if let Flow::Applied { version } = &self.flow {
            return Some((
                format!("Updated to v{version} — switching when you're away").into(),
                StripAction::Restart,
            ));
        }
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
            Flow::Applied { version } => (
                format!("Updated to v{version} — switching when you're away").into(),
                StripAction::Restart,
            ),
            Flow::Swapped { version } => (
                format!("Updated to v{version}").into(),
                StripAction::DismissUpdated,
            ),
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

/// "Check for Updates…" from a menu: surface the main window, then check.
pub fn check_for_updates(cx: &mut App) {
    crate::activate_main_window(cx);
    if let Some(update) = AppUpdate::global(cx) {
        update.update(cx, |update, cx| update.check_for_updates(cx));
    }
}

fn swap_marker_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ui-updated.json")
}

/// Leave a note for the new window: "you are what an update just installed".
fn write_swap_marker(data_dir: &Path, version: &str) -> anyhow::Result<()> {
    std::fs::write(
        swap_marker_path(data_dir),
        serde_json::json!({ "version": version }).to_string(),
    )?;
    Ok(())
}

/// Read and remove the note [`write_swap_marker`] left; only a window that
/// really is that version reports "Updated to vX".
fn take_swap_marker(data_dir: &Path, running: &str) -> Option<String> {
    let path = swap_marker_path(data_dir);
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let version = serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get("version")?
        .as_str()?
        .to_owned();
    (version == running).then_some(version)
}

/// Environment marking a window started by a swap: it attaches to the running
/// engine host (never embeds one) and announces itself when it is up. The value
/// is `background` when the user was elsewhere at the time (the new window must
/// not take the focus).
pub const SWAP_ENV: &str = "ZERON_UI_SWAP";

static START: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// Read how this process was started (see [`SWAP_ENV`]) and REMOVE the
/// variables from the environment: this window's children (an engine host, its
/// agents and terminals, an external editor) must never inherit them. Call
/// once, first thing, while single-threaded.
pub fn capture_start_env() {
    let value = std::env::var(SWAP_ENV).ok();
    let attach = std::env::var("ZERON_ATTACH_ONLY").is_ok();
    // SAFETY: the caller runs this before any thread exists.
    unsafe {
        std::env::remove_var(SWAP_ENV);
        std::env::remove_var("ZERON_ATTACH_ONLY");
    }
    let _ = START.set(value.or(attach.then(|| "1".to_string())));
}

/// This window was started by an update swap.
pub fn started_by_swap() -> bool {
    START.get().is_some_and(Option::is_some)
}

/// ...and the user was not looking at the old window when it did.
pub fn started_in_background() -> bool {
    START
        .get()
        .and_then(Option::as_deref)
        .is_some_and(|value| value == "background")
}

fn ready_path(data_dir: &Path, pid: u32) -> PathBuf {
    data_dir.join(format!("ui-ready-{pid}"))
}

/// Start the new build of the window beside this one. `stable_exe` is the
/// path the update just replaced, so this is the NEW binary.
pub fn spawn_new_ui(background: bool) -> anyhow::Result<std::process::Child> {
    let exe = zeron_update::stable_exe()?;
    let mut command = std::process::Command::new(exe);
    command
        .env(SWAP_ENV, if background { "background" } else { "1" })
        .stdin(std::process::Stdio::null());
    // Its own session: a terminal the old window was started from must not be
    // able to interrupt or hang up the new one.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and touches no shared state.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    Ok(command.spawn()?)
}

/// Wait for the new window to announce itself (it touches `ui-ready-<pid>`).
/// `false` on timeout or if it died first.
pub async fn wait_for_new_ui(
    data_dir: &Path,
    child: &mut std::process::Child,
    timeout: Duration,
) -> bool {
    let ready = ready_path(data_dir, child.id());
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if ready.exists() {
            let _ = std::fs::remove_file(&ready);
            return true;
        }
        if matches!(child.try_wait(), Ok(Some(_))) || tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A window a swap started calls this once it is attached and showing: it is
/// what the old window waits for before it goes away.
pub fn signal_ready(cx: &App) {
    if !started_by_swap() {
        return;
    }
    if let Some(update) = AppUpdate::global(cx) {
        let data_dir = update.read(cx).data_dir().to_path_buf();
        let _ = std::fs::write(ready_path(&data_dir, std::process::id()), b"");
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

    fn gate(active: bool, idle: u64, composing: bool, unsaved: bool) -> SwapGate {
        SwapGate {
            window_active: active,
            idle_for: Duration::from_secs(idle),
            composing,
            unsaved_edits: unsaved,
        }
    }

    fn ready() -> Flow {
        Flow::Ready {
            version: "0.3.0".into(),
            staged: PathBuf::from("/x"),
        }
    }

    #[test]
    fn ui_swaps_only_when_the_window_is_unfocused_or_idle_and_nothing_is_half_done() {
        assert!(may_swap_now(&gate(false, 0, false, false)));
        assert!(may_swap_now(&gate(true, 61, false, false)));
        assert!(!may_swap_now(&gate(true, 10, false, false)));
        assert!(!may_swap_now(&gate(true, 59, false, false)));
        assert!(
            !may_swap_now(&gate(false, 999, true, false)),
            "IME composition"
        );
        assert!(
            !may_swap_now(&gate(false, 999, false, true)),
            "unsaved file"
        );
        assert!(!may_swap_now(&gate(true, 999, true, true)));
    }

    #[test]
    fn a_ready_update_installs_without_a_prompt_when_auto_update_is_on() {
        let on = Settings { auto_update: true };
        let off = Settings { auto_update: false };
        let quiet = gate(false, 0, false, false);
        assert_eq!(decide("linux", &ready(), on, quiet, true), Action::Install);
        assert_eq!(decide("macos", &ready(), on, quiet, true), Action::Install);
        assert_eq!(
            decide("linux", &ready(), off, quiet, true),
            Action::ShowStrip
        );
        // Busy: keep asking, never interrupt.
        assert_eq!(
            decide("linux", &ready(), on, gate(true, 5, false, false), true),
            Action::Wait
        );
        assert_eq!(
            decide("linux", &ready(), on, gate(false, 0, true, false), true),
            Action::Wait
        );
        // Nothing staged, nothing to do.
        assert_eq!(
            decide("linux", &Flow::Idle, on, quiet, true),
            Action::Nothing
        );
    }

    #[test]
    fn the_strip_reports_updated_not_restart_to_update() {
        let swapped = Flow::Swapped {
            version: "0.3.0".into(),
        };
        assert_eq!(strip_text(&swapped).as_deref(), Some("Updated to v0.3.0"));
        assert_eq!(strip_text(&ready()), None);
        let managed = InstallKind::Managed {
            app_root: PathBuf::from("/a"),
        };
        assert_eq!(
            strip_for(&managed, false, &swapped, "0.3.0").1,
            StripAction::DismissUpdated
        );
    }

    #[test]
    fn an_installed_update_only_waits_for_its_window_swap() {
        let applied = Flow::Applied {
            version: "0.3.0".into(),
        };
        let on = Settings { auto_update: true };
        // Same gate as a freshly staged one: quiet moments swap, busy ones wait.
        assert_eq!(
            decide("linux", &applied, on, gate(false, 0, false, false), true),
            Action::Install
        );
        assert_eq!(
            decide("linux", &applied, on, gate(true, 5, false, false), true),
            Action::Wait
        );
        let managed = InstallKind::Managed {
            app_root: PathBuf::from("/a"),
        };
        let (label, action) = strip_for(&managed, false, &applied, "0.3.0");
        assert!(label.contains("Updated to v0.3.0"), "{label}");
        assert_eq!(action, StripAction::Restart, "clicking switches now");
    }

    #[test]
    fn windows_keeps_install_on_quit_but_never_prompts() {
        assert_eq!(
            decide(
                "windows",
                &ready(),
                Settings { auto_update: true },
                gate(false, 0, false, false),
                true
            ),
            Action::InstallOnQuit
        );
        // Report-only still means report-only there.
        assert_eq!(
            decide(
                "windows",
                &ready(),
                Settings { auto_update: false },
                gate(false, 0, false, false),
                true
            ),
            Action::ShowStrip
        );
    }

    #[test]
    fn zeron_auto_update_zero_keeps_report_only() {
        assert_eq!(
            decide(
                "linux",
                &ready(),
                Settings { auto_update: true },
                gate(false, 0, false, false),
                false
            ),
            Action::ShowStrip
        );
    }

    #[test]
    fn the_new_window_learns_it_was_just_updated_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        write_swap_marker(dir.path(), "0.3.0").unwrap();
        assert_eq!(
            take_swap_marker(dir.path(), "0.3.0").as_deref(),
            Some("0.3.0")
        );
        assert_eq!(take_swap_marker(dir.path(), "0.3.0"), None, "consumed");
        // A window that is not the installed version does not claim it.
        write_swap_marker(dir.path(), "0.3.0").unwrap();
        assert_eq!(take_swap_marker(dir.path(), "0.2.9"), None);
        assert!(!swap_marker_path(dir.path()).exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_old_window_waits_for_the_new_one_to_announce_itself() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let ready = ready_path(dir.path(), child.id());
        let announce = ready.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            std::fs::write(announce, b"").unwrap();
        });
        assert!(wait_for_new_ui(dir.path(), &mut child, Duration::from_secs(5)).await);
        assert!(!ready.exists(), "the signal is consumed");
        // Nobody announces: time out and report it.
        assert!(!wait_for_new_ui(dir.path(), &mut child, Duration::from_millis(300)).await);
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_new_window_that_dies_before_announcing_is_reported_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let started = std::time::Instant::now();
        assert!(!wait_for_new_ui(dir.path(), &mut child, Duration::from_secs(20)).await);
        assert!(started.elapsed() < Duration::from_secs(5));
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
