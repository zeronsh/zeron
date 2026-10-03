//! The unified Updates surface: Zeron itself (the app on this Mac, a separate
//! local daemon, remote engines) beside every device's agent CLIs.
//!
//! Each lifecycle keeps its owner. The desktop app's download/restart flow is
//! [`crate::app_update::AppUpdate`]; engine operations belong to the engine
//! that runs them (`engine-updates-v1`); agent CLIs belong to their device's
//! coordinator. This module only watches them and turns their states into
//! rows keyed by device plus component, so nothing here checks, downloads or
//! installs on its own.

use super::*;
use zeron_proto::{
    EngineUpdateAck, EngineUpdatePhase as EnginePhase, EngineUpdateState, EngineUpdateSupport,
    HarnessId,
};

/// How rows name the device this window runs on.
pub(super) const THIS_DEVICE: &str = if cfg!(target_os = "macos") {
    "This Mac"
} else {
    "This device"
};

/// Release data older than this is refreshed by asking the device to check.
const REFRESH_STALE: Duration = Duration::from_secs(30 * 60);
/// How often held data is re-examined for staleness. Requests still only go
/// out once the data is stale, so a device is asked at most every ~30 minutes.
pub(super) const REFRESH_POLL: Duration = Duration::from_secs(5 * 60);

/// Whether a check timestamp (epoch ms) is missing or old enough to refresh.
pub(super) fn refresh_due(checked_at: Option<i64>, now_ms: i64) -> bool {
    checked_at.is_none_or(|at| now_ms.saturating_sub(at) > REFRESH_STALE.as_millis() as i64)
}

/// A restarted engine normally reconnects within seconds. After this long,
/// stop implying progress and say what is known.
const RECONNECT_TIMEOUT: Duration = Duration::from_secs(3 * 60);
/// How long a completed Zeron update stays on Home.
const UPDATED_NOTICE: Duration = Duration::from_secs(2 * 60);

/// What a device's engine has told us about its own installation.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) enum EngineSource {
    /// Not heard from yet.
    #[default]
    Unknown,
    /// `engine-updates-v1`: full operation lifecycle.
    Rich(EngineUpdateState),
    /// Only the version facts of the original `UpdateStatus` stream.
    Legacy(zeron_update::UpdateStatus),
    /// The engine answers neither update method.
    NoUpdateRpc,
}

#[derive(Default)]
pub(super) struct EngineDevice {
    pub(super) online: bool,
    /// The watch stream is live and has delivered a fresh frame.
    pub(super) connected: bool,
    /// Which protocol the running watch speaks. A registry row that gains the
    /// capability after an upgrade restarts the watch in rich mode.
    pub(super) rich: bool,
    pub(super) source: EngineSource,
    /// Set when the stream ended while an operation was restarting.
    pub(super) reconnecting_since: Option<std::time::Instant>,
    /// A start request in flight from this window (double-click guard).
    pub(super) starting: bool,
    pub(super) watch: Option<Task<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Component {
    /// The visible desktop executable.
    App,
    /// This Mac's separate daemon, or a remote device's engine.
    Engine,
    Agent(HarnessId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    Normal,
    Active,
    Done,
    Danger,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RowAction {
    Start,
    Cancel {
        operation: String,
    },
    Check,
    /// The row's explanation is the instruction; open the Updates page.
    Instructions,
}

/// Why the last action on a row failed, by device and component. Kept until
/// that row is acted on again, its owner reports new state, or Home's offers
/// are dismissed.
pub(super) type ActionErrors = std::collections::HashMap<(String, Component), String>;

/// One row of the Updates surface.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ZeronRow {
    pub(super) device_id: String,
    pub(super) device_name: String,
    pub(super) component: Component,
    pub(super) title: String,
    pub(super) detail: String,
    /// Longer explanation (instructions, full error) for tooltips/Settings.
    pub(super) explanation: Option<String>,
    pub(super) tone: Tone,
    pub(super) action: Option<(&'static str, RowAction)>,
    /// Actionable or in progress: Home shows it.
    pub(super) notice: bool,
    pub(super) connected: bool,
    /// What is on offer (a version, or a marker for versionless updates).
    /// Dismissing is per offer, so a newer release shows again.
    pub(super) offer: String,
}

impl ZeronRow {
    /// Identity of this row's current offer, for dismissal.
    pub(super) fn dismiss_key(&self) -> String {
        format!("{}|{:?}|{}", self.device_id, self.component, self.offer)
    }

    /// Show why the last action on this row failed, on whichever surface the
    /// row appears. The action stays, so it can be tried again.
    pub(super) fn with_action_error(mut self, errors: &ActionErrors) -> Self {
        if let Some(error) = errors.get(&(self.device_id.clone(), self.component)) {
            self.detail = "Request failed".into();
            self.explanation = Some(error.clone());
            self.tone = Tone::Danger;
            self.notice = true;
        }
        self
    }

    /// A dismissed offer stays hidden only while it is just an offer. Work
    /// in progress and failures must never disappear behind a dismissal.
    pub(super) fn hidden_by(&self, dismissed: &std::collections::HashSet<String>) -> bool {
        matches!(self.tone, Tone::Normal | Tone::Done) && dismissed.contains(&self.dismiss_key())
    }
}

/// Zeron rows are identified by where they run. The pixel-grid logo turns
/// into a grey block at mark sizes, so use the device glyphs instead.
pub(super) fn zeron_mark(row: &ZeronRow) -> &'static str {
    if row.device_name == THIS_DEVICE {
        icons::MONITOR
    } else {
        icons::REMOTE_SERVER
    }
}

/// Instructions for engines that cannot be updated from here. Specific to
/// the unsupported capability, never a generic "update failed".
fn legacy_instructions(running: &str) -> String {
    format!(
        "This engine runs Zeron {running}, which can't install updates on request. Run \
         `zeron update` on that device once; later versions can be updated from here."
    )
}

fn engine_title(local: bool) -> String {
    if local {
        "Zeron engine".into()
    } else {
        "Zeron".into()
    }
}

/// The presentation of one device's engine. Pure: every input is explicit.
pub(super) fn engine_row(
    device_id: &str,
    device_name: &str,
    local: bool,
    device: &EngineDevice,
    now: std::time::Instant,
    now_ms: i64,
) -> ZeronRow {
    let mut row = ZeronRow {
        device_id: device_id.to_owned(),
        device_name: device_name.to_owned(),
        component: Component::Engine,
        title: engine_title(local),
        detail: String::new(),
        explanation: None,
        tone: Tone::Normal,
        action: None,
        notice: false,
        connected: device.online && device.connected,
        offer: String::new(),
    };
    row.offer = match &device.source {
        EngineSource::Rich(state) => state
            .operation
            .as_ref()
            .and_then(|op| op.target_version.clone())
            .or_else(|| state.latest_version.clone()),
        EngineSource::Legacy(status) => status.latest_version.clone(),
        _ => None,
    }
    .unwrap_or_default();
    match &device.source {
        EngineSource::Unknown => {
            row.detail = if device.online {
                "Checking…".into()
            } else {
                "Offline".into()
            };
        }
        EngineSource::NoUpdateRpc => {
            row.detail = "Can't report updates".into();
            row.explanation = Some(
                "This engine predates remote update reporting. Run `zeron update` on that \
                 device."
                    .into(),
            );
            row.action = Some(("View instructions", RowAction::Instructions));
        }
        EngineSource::Legacy(status) => {
            let latest = status
                .latest_version
                .as_deref()
                .filter(|_| status.update_available);
            match latest {
                Some(latest) => {
                    row.detail = format!("{} → {latest}", status.current_version);
                    row.explanation = Some(legacy_instructions(&status.current_version));
                    row.action = Some(("View instructions", RowAction::Instructions));
                    row.notice = true;
                }
                None => row.detail = format!("Up to date · {}", status.current_version),
            }
            if !row.connected {
                row.detail = format!("Offline · {}", row.detail);
            }
        }
        EngineSource::Rich(state) => rich_row(&mut row, state, device, now, now_ms),
    }
    row
}

fn rich_row(
    row: &mut ZeronRow,
    state: &EngineUpdateState,
    device: &EngineDevice,
    now: std::time::Instant,
    now_ms: i64,
) {
    let running = state.running_version.as_str();
    let operation = state.operation.as_ref();
    let target = operation
        .and_then(|op| op.target_version.as_deref())
        .or(state.latest_version.as_deref())
        .unwrap_or("the update");

    // Disconnected mid-restart: the restart is expected to drop the stream.
    // Only the running version, read after reconnecting, proves success.
    if !row.connected
        && let Some(op) = operation.filter(|op| op.phase == EnginePhase::Restarting)
    {
        let waited = device
            .reconnecting_since
            .map(|since| now.saturating_duration_since(since))
            .unwrap_or_default();
        row.notice = true;
        if waited < RECONNECT_TIMEOUT {
            row.detail = format!("Reconnecting after installing {target}…");
            row.tone = Tone::Active;
        } else {
            row.detail = "Didn't reconnect after restarting".into();
            row.explanation = Some(format!(
                "Zeron installed {target} on {} and asked its service manager to restart, \
                 but the engine hasn't reconnected. It was running {}. Check the device.",
                row.device_name, op.from_version
            ));
            row.tone = Tone::Danger;
            row.action = Some(("View instructions", RowAction::Instructions));
        }
        return;
    }

    if let Some(op) = operation {
        let in_flight = |detail: String, cancellable: bool, row: &mut ZeronRow| {
            row.detail = detail;
            row.tone = Tone::Active;
            row.notice = true;
            if cancellable {
                row.action = Some((
                    "Cancel",
                    RowAction::Cancel {
                        operation: op.id.clone(),
                    },
                ));
            }
        };
        match op.phase {
            EnginePhase::Staging => {
                return in_flight(format!("Downloading {target}…"), true, row);
            }
            EnginePhase::WaitingForIdle => {
                // Already on disk (someone else installed it): only the
                // restart is waiting.
                let waiting = if state.restart_pending() {
                    format!("Restarts into {target} when runs and terminals finish")
                } else {
                    format!("{target} installs when runs and terminals finish")
                };
                return in_flight(waiting, true, row);
            }
            EnginePhase::Applying => {
                return in_flight(format!("Installing {target}…"), false, row);
            }
            EnginePhase::Restarting => return in_flight("Restarting…".into(), false, row),
            EnginePhase::Updated => {
                row.detail = format!("Updated to {running}");
                row.tone = Tone::Done;
                row.notice = now_ms.saturating_sub(op.updated_at)
                    < UPDATED_NOTICE.as_millis() as i64
                    && !state.update_available();
                if row.notice || !state.update_available() {
                    return;
                }
            }
            EnginePhase::Failed => {
                row.detail = op
                    .error
                    .clone()
                    .unwrap_or_else(|| "The update failed".into());
                row.explanation = op.error.clone();
                row.tone = Tone::Danger;
                row.notice = true;
                // A failed operation stays the engine's latest until another
                // starts. While its release is still on offer, retry it;
                // "Check again" alone would never bring back an Update button.
                row.action = if state.update_available() && state.support.can_install() {
                    Some(("Try again", RowAction::Start))
                } else {
                    Some(("Check again", RowAction::Check))
                };
                if !row.connected {
                    row.action = None;
                }
                return;
            }
            // Cancelled and unknown phases fall through to the version facts.
            EnginePhase::RestartRequired | EnginePhase::Cancelled | EnginePhase::Unknown => {}
        }
    }

    if state.restart_pending() {
        row.detail = format!(
            "{} installed · restart required",
            state.installed_version.as_deref().unwrap_or(target)
        );
        row.explanation = Some(
            operation
                .and_then(|op| op.error.clone())
                .or_else(|| match &state.support {
                    EngineUpdateSupport::ManualRestart { reason } => Some(reason.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| {
                    "Restart Zeron on that device to finish the update. Runs and terminals \
                     end when it restarts."
                        .into()
                }),
        );
        row.tone = Tone::Normal;
        row.notice = true;
        row.action = Some(("Restart required", RowAction::Instructions));
        return;
    }

    if state.update_available() {
        let latest = state.latest_version.as_deref().unwrap_or_default();
        row.detail = format!("{running} → {latest}");
        row.notice = true;
        match &state.support {
            EngineUpdateSupport::Managed => {
                row.action = Some(("Update when idle", RowAction::Start));
            }
            EngineUpdateSupport::ManualRestart { reason } => {
                row.explanation = Some(reason.clone());
                row.action = Some(("Update", RowAction::Start));
            }
            EngineUpdateSupport::Unsupported { reason } => {
                row.explanation = Some(reason.clone());
                row.action = Some(("View instructions", RowAction::Instructions));
            }
            EngineUpdateSupport::Unknown => {
                row.explanation =
                    Some("This engine reports an update mode this app doesn't know.".into());
                row.action = Some(("View instructions", RowAction::Instructions));
            }
        }
    } else if let Some(error) = state
        .check_error
        .as_ref()
        .filter(|_| state.latest_version.is_none())
    {
        row.detail = "Couldn't check for updates".into();
        row.explanation = Some(error.clone());
        row.action = Some(("Check again", RowAction::Check));
    } else if state.checked_at.is_none() {
        row.detail = format!("{running} · not checked yet");
        row.action = Some(("Check for updates", RowAction::Check));
    } else {
        row.detail = format!("Up to date · {running}");
        if let EngineUpdateSupport::Unsupported { reason } = &state.support {
            row.explanation = Some(reason.clone());
        }
    }
    if !row.connected {
        row.detail = format!("Offline · {}", row.detail);
        // Last-known facts stay visible; actions wait for a live engine.
        if !matches!(row.action, Some((_, RowAction::Instructions))) {
            row.action = None;
        }
    }
}

/// The desktop app's own row, from its update owner. `None` when the app has
/// nothing to say (no release found yet).
pub(super) fn app_row(update: &crate::app_update::AppUpdate, device_id: &str) -> ZeronRow {
    use crate::app_update::{Flow, StripAction};
    let current = zeron_update::current_version();
    let mut row = ZeronRow {
        device_id: device_id.to_owned(),
        device_name: THIS_DEVICE.into(),
        component: Component::App,
        title: "Zeron app".into(),
        detail: format!("Up to date · {current}"),
        explanation: None,
        tone: Tone::Normal,
        action: None,
        notice: false,
        connected: true,
        offer: update.available().unwrap_or_default().to_owned(),
    };
    if update
        .status()
        .is_none_or(|status| status.checked_at.is_none())
    {
        row.detail = format!("{current} · not checked yet");
        row.action = Some(("Check for updates", RowAction::Check));
    }
    if let Some(error) = update.status().and_then(|status| status.error.as_ref()) {
        row.detail = format!("{current} · couldn't check for updates");
        row.explanation = Some(error.clone());
        row.action = Some(("Check again", RowAction::Check));
    }
    if let Some(latest) = update.available() {
        let (_, action) = crate::app_update::strip_for(
            update.install(),
            update.blocker().is_some(),
            update.flow(),
            latest,
        );
        row.notice = true;
        row.detail = match update.flow() {
            Flow::Downloading { version } => {
                row.tone = Tone::Active;
                format!("Downloading {version}…")
            }
            Flow::Ready { version, .. } => format!("{version} ready · restart to apply"),
            Flow::Failed { message, .. } => {
                row.tone = Tone::Danger;
                row.explanation = Some(message.to_string());
                format!("Update failed: {message}")
            }
            Flow::Installed => {
                row.tone = Tone::Active;
                "Restarting…".into()
            }
            Flow::Idle => format!("{current} → {latest}"),
        };
        if let Some(blocker) = update.blocker() {
            row.explanation = Some(blocker.to_string());
        }
        row.action = match action {
            StripAction::None => None,
            StripAction::Download => Some(("Update", RowAction::Start)),
            StripAction::Restart => Some(("Restart app", RowAction::Start)),
            StripAction::Explain | StripAction::Advise { .. } => {
                Some(("View instructions", RowAction::Instructions))
            }
        };
    }
    row
}

// Reconcile the engine inventory: presence changes never discard last-known
// state; departed devices leave; capability changes restart the watch.
pub(super) fn reconcile_engines(
    devices: &mut std::collections::BTreeMap<String, EngineDevice>,
    desired: &std::collections::BTreeMap<String, (bool, bool)>,
) -> Vec<String> {
    devices.retain(|id, _| desired.contains_key(id));
    let mut start = Vec::new();
    for (id, (online, rich)) in desired {
        let device = devices.entry(id.clone()).or_default();
        device.online = *online;
        if device.watch.is_some() && device.rich != *rich {
            device.watch = None;
        }
        device.rich = *rich;
        if !online {
            device.watch = None;
            device.connected = false;
            // Presence can report the restart before the watch notices its
            // stream ended; the reconnect timeout must still start.
            let restarting = matches!(&device.source, EngineSource::Rich(state)
                if state.operation.as_ref().is_some_and(|op| op.phase == EnginePhase::Restarting));
            if restarting && device.reconnecting_since.is_none() {
                device.reconnecting_since = Some(std::time::Instant::now());
            }
        } else if device.watch.is_none() {
            start.push(id.clone());
        }
    }
    start
}

impl Shell {
    /// Devices whose engine rows the Updates surface shows: every registered
    /// device, plus this Mac's daemon when it is a separate process. An
    /// in-process engine shares the app's installation and restart boundary,
    /// so the app row already covers it.
    fn engine_update_inventory(
        &self,
        cx: &App,
    ) -> std::collections::BTreeMap<String, (bool, bool)> {
        let state = self.state.read(cx);
        let Some(engine) = state.engine() else {
            return Default::default();
        };
        let local = engine.engine_info().device_id.clone();
        let embedded = matches!(engine.mode(), crate::state::EngineMode::InProcess);
        state
            .devices
            .iter()
            .map(|device| device.id.clone())
            .chain(std::iter::once(local.clone()))
            .filter(|id| !(embedded && *id == local))
            .map(|id| {
                let online = state.device_online(&id, Utc::now());
                let rich = state.device_supports(&id, zeron_proto::capabilities::ENGINE_UPDATES_V1);
                (id, (online, rich))
            })
            .collect()
    }

    pub(super) fn refresh_engine_update_watch(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        if !matches!(state.connection, ConnectionStatus::Ready) {
            self.engine_update_devices.clear();
            return;
        }
        let Some(engine) = state.engine().cloned() else {
            return;
        };
        let desired = self.engine_update_inventory(cx);
        for device in reconcile_engines(&mut self.engine_update_devices, &desired) {
            let entry = self.engine_update_devices.get_mut(&device).unwrap();
            let rich = entry.rich;
            let engine = engine.clone();
            entry.watch = Some(cx.spawn(async move |this, cx| {
                watch_engine(this, engine, device, rich, cx).await;
            }));
        }
    }

    /// The device id the app's own row is filed under.
    pub(super) fn local_update_device(&self, cx: &App) -> String {
        let state = self.state.read(cx);
        state
            .engine()
            .map(|engine| engine.engine_info().device_id.clone())
            .unwrap_or_default()
    }

    /// Every Zeron row, in a stable order: this Mac first, then devices by name.
    pub(super) fn zeron_update_rows(&self, include_app: bool, cx: &App) -> Vec<ZeronRow> {
        let state = self.state.read(cx);
        let local = state.engine().map(|e| e.engine_info().device_id.clone());
        let now = std::time::Instant::now();
        let now_ms = Utc::now().timestamp_millis();
        let mut rows = Vec::new();
        if include_app && let Some(update) = crate::app_update::AppUpdate::global(cx) {
            rows.push(app_row(
                update.read(cx),
                local.as_deref().unwrap_or_default(),
            ));
        }
        let mut engines: Vec<_> = self
            .engine_update_devices
            .iter()
            .map(|(id, device)| {
                let is_local = local.as_deref() == Some(id.as_str());
                let name = if is_local {
                    THIS_DEVICE.to_owned()
                } else {
                    state.device_name(id).unwrap_or(id).to_owned()
                };
                engine_row(id, &name, is_local, device, now, now_ms)
            })
            .collect();
        engines.sort_by_key(|row| {
            (
                row.device_name != THIS_DEVICE,
                row.device_name.to_lowercase(),
            )
        });
        rows.extend(engines);
        rows.into_iter()
            .map(|row| row.with_action_error(&self.update_action_errors))
            .collect()
    }

    pub(super) fn run_engine_update_action(
        &mut self,
        device: String,
        action: RowAction,
        cx: &mut Context<Self>,
    ) {
        if matches!(action, RowAction::Instructions) {
            self.set_harness_updates_expanded(false, cx);
            self.open_settings(SettingsSection::Updates, cx);
            return;
        }
        let state = self.state.read(cx);
        let Some(engine) = state.engine().cloned() else {
            return;
        };
        let Some(entry) = self.engine_update_devices.get_mut(&device) else {
            return;
        };
        if !(entry.online && entry.connected) {
            return;
        }
        self.update_action_errors
            .remove(&(device.clone(), Component::Engine));
        let (method, params) = match &action {
            RowAction::Start => {
                if entry.starting {
                    return;
                }
                entry.starting = true;
                // One idempotency key per click: a retried delivery joins the
                // engine's operation instead of starting a second one.
                let request = uuid::Uuid::new_v4().to_string();
                (
                    methods::START_ENGINE_UPDATE,
                    serde_json::json!({ "requestId": request, "targetDeviceId": device }),
                )
            }
            RowAction::Cancel { operation } => (
                methods::CANCEL_ENGINE_UPDATE,
                serde_json::json!({ "operationId": operation, "targetDeviceId": device }),
            ),
            RowAction::Check => (
                methods::CHECK_ENGINE_UPDATE,
                serde_json::json!({ "targetDeviceId": device }),
            ),
            RowAction::Instructions => unreachable!(),
        };
        cx.spawn(async move |this, cx| {
            let result = engine.client().call(method, params).await;
            this.update(cx, |shell, cx| {
                if let Some(entry) = shell.engine_update_devices.get_mut(&device) {
                    entry.starting = false;
                    // The ack carries the accepted operation. Apply it now so
                    // the row moves even before the watch's next frame.
                    if method == methods::START_ENGINE_UPDATE
                        && let Ok(value) = &result
                        && let Ok(ack) = serde_json::from_value::<EngineUpdateAck>(value.clone())
                    {
                        entry.source = EngineSource::Rich(ack.state);
                    }
                }
                if let Err(error) = result {
                    shell
                        .update_action_errors
                        .insert((device, Component::Engine), error.to_string());
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn run_app_update_action(&mut self, cx: &mut Context<Self>) {
        let Some(update) = crate::app_update::AppUpdate::global(cx) else {
            return;
        };
        let key = (self.local_update_device(cx), Component::App);
        self.update_action_errors.remove(&key);
        let (latest, install, blocked, flow) = {
            let update = update.read(cx);
            (
                update.available().map(str::to_owned),
                update.install().clone(),
                update.blocker().is_some(),
                update.flow().clone(),
            )
        };
        if let Some(latest) = latest {
            let (_, action) = crate::app_update::strip_for(&install, blocked, &flow, &latest);
            self.on_update_strip_click(action, cx);
        }
    }

    /// Settings → Updates: every installation this window knows about,
    /// grouped by device, including current, offline and unsupported ones.
    pub(super) fn render_updates_settings(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.refresh_harness_update_watch(cx);
        let theme = Theme::of(cx).for_settings_surface();
        let zeron = self.zeron_update_rows(true, cx);
        let local = self
            .state
            .read(cx)
            .engine()
            .map(|e| e.engine_info().device_id.clone());
        let agents = self.agent_update_rows(false, cx);
        let mut groups: Vec<(String, String, Vec<ZeronRow>)> = Vec::new();
        for row in zeron.into_iter().chain(agents) {
            let key = if row.component == Component::App {
                local.clone().unwrap_or_default()
            } else {
                row.device_id.clone()
            };
            match groups.iter_mut().find(|(id, _, _)| *id == key) {
                Some((_, _, rows)) => rows.push(row),
                None => groups.push((key, row.device_name.clone(), vec![row])),
            }
        }
        let mut column = settings::widgets::page_column()
            .child(settings::widgets::page_header(&theme, "Updates", None))
            .child(settings::widgets::page_subtitle(
                &theme,
                "Zeron and agent CLIs on each device. Each update runs on the device it \
                 changes, and Zeron restarts only when nothing is running there.",
            ));
        for (index, (_, name, rows)) in groups.into_iter().enumerate() {
            let mut block = settings::widgets::section_card(&theme).mt(px(0.0));
            for (position, row) in rows.into_iter().enumerate() {
                block = block.child(self.render_update_settings_row(
                    row,
                    position == 0,
                    (index, position),
                    &theme,
                    cx,
                ));
            }
            column = column.child(settings::widgets::section(&theme, name, block));
        }
        let rail = settings::widgets::rail(
            &mut self.updates_page_scroll,
            "updates-page-scrollbar",
            &theme,
            cx,
            |shell| &mut shell.updates_page_scroll,
        );
        div()
            .id("updates-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.updates_page_scroll.set_list_hovered(*hovered) {
                    cx.notify();
                }
            }))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("updates-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.updates_page_scroll.scroll)
                        .child(column),
                )
                .fade_overflow_y(&self.updates_page_scroll.scroll),
            )
            .children(rail)
            .into_any_element()
    }

    fn render_update_settings_row(
        &mut self,
        row: ZeronRow,
        first: bool,
        key: (usize, usize),
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (mark, tint) = match row.component {
            Component::Agent(harness) => crate::pickers::harness_brand_icon(harness),
            Component::App | Component::Engine => (zeron_mark(&row), None),
        };
        let detail_color = match row.tone {
            Tone::Danger => theme.danger,
            Tone::Done => theme.success,
            Tone::Normal | Tone::Active => theme.text_muted,
        };
        let action = row
            .action
            .clone()
            // An engine's instructions are already on this page, under the
            // row. App and agent instructions lead somewhere else.
            .filter(|(_, action)| {
                row.component != Component::Engine || !matches!(action, RowAction::Instructions)
            })
            .map(|(label, action)| {
                let device = row.device_id.clone();
                let component = row.component;
                let enabled = row.connected;
                settings::widgets::text_action(theme, settings::widgets::ActionTone::Filled, label)
                    .id(SharedString::from(format!(
                        "settings-update-action-{}-{}",
                        key.0, key.1
                    )))
                    .opacity(if enabled { 1.0 } else { 0.45 })
                    .role(gpui::Role::Button)
                    .aria_label(format!("{label} · {} · {}", row.title, row.device_name))
                    .tab_index(if enabled { 0 } else { -1 })
                    .focus_visible({
                        let accent = theme.accent;
                        move |s| s.border_2().border_color(accent)
                    })
                    .when(enabled, |button| {
                        button.on_click(cx.listener(move |this, _, _, cx| {
                            this.run_update_row_action(
                                device.clone(),
                                component,
                                action.clone(),
                                cx,
                            );
                        }))
                    })
            });
        let busy = (row.tone == Tone::Active).then(|| {
            loaders::mini_glyph_spinner(
                SharedString::from(format!("settings-update-busy-{}-{}", key.0, key.1)),
                2.0,
                theme.glyph,
                cx.entity_id(),
                cx,
            )
        });
        settings::widgets::card_row(theme, first)
            .flex_nowrap()
            .child(
                icon(mark)
                    .size(px(18.0))
                    .flex_none()
                    .text_color(tint.unwrap_or(theme.text_muted)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(settings::widgets::row_title(theme, row.title.clone()))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(
                                settings::widgets::ROW_DESCRIPTION_SIZE,
                            ))
                            .text_color(detail_color)
                            .child(row.detail.clone()),
                    )
                    // Instructions wrap rather than clip: this page is where
                    // "View instructions" lands.
                    .when_some(row.explanation.clone(), |el, text| {
                        el.child(
                            div()
                                .text_size(crate::typography::ui_rems(
                                    settings::widgets::ROW_DESCRIPTION_SIZE,
                                ))
                                .text_color(theme.text_muted)
                                .child(text),
                        )
                    }),
            )
            .children(busy)
            .children(action)
            .into_any_element()
    }

    pub(super) fn run_update_row_action(
        &mut self,
        device: String,
        component: Component,
        action: RowAction,
        cx: &mut Context<Self>,
    ) {
        match component {
            Component::App => match action {
                RowAction::Check => {
                    if let Some(update) = crate::app_update::AppUpdate::global(cx) {
                        update.update(cx, |update, cx| update.check_for_updates(cx));
                    }
                }
                RowAction::Instructions if !matches!(self.route, Route::Settings(_)) => {
                    self.open_settings(SettingsSection::Updates, cx);
                }
                _ => self.run_app_update_action(cx),
            },
            Component::Engine => self.run_engine_update_action(device, action, cx),
            Component::Agent(harness) => match action {
                RowAction::Instructions => self.open_harness_update_steps(device, cx),
                RowAction::Start => self.run_harness_update_action(
                    device,
                    methods::APPLY_HARNESS_UPDATE,
                    harness,
                    cx,
                ),
                RowAction::Cancel { .. } => self.run_harness_update_action(
                    device,
                    methods::CANCEL_HARNESS_UPDATE,
                    harness,
                    cx,
                ),
                RowAction::Check => self.run_harness_update_action(
                    device,
                    methods::CHECK_HARNESS_UPDATES,
                    harness,
                    cx,
                ),
            },
        }
    }
}

/// One device's engine watch. Rich engines stream their lifecycle; older
/// ones are probed once for the original version stream. An unsupported
/// method is a state, not something to retry forever.
async fn watch_engine(
    this: gpui::WeakEntity<Shell>,
    engine: crate::state::EngineHandle,
    device: String,
    rich: bool,
    cx: &mut gpui::AsyncApp,
) {
    let mut retry = 1;
    let mut rich = rich;
    // Engines check hourly on their own; don't leave Home waiting on that.
    // Legacy engines have no way to be asked, so only rich ones are refreshed.
    let _refresher = cx.spawn({
        let this = this.clone();
        let engine = engine.clone();
        let device = device.clone();
        async move |cx| {
            let mut delay = Duration::from_secs(3);
            loop {
                cx.background_executor().timer(delay).await;
                delay = REFRESH_POLL;
                let due = this.update(cx, |shell, _| {
                    let now = Utc::now().timestamp_millis();
                    shell
                        .engine_update_devices
                        .get(&device)
                        .is_some_and(|entry| {
                            entry.online
                                && entry.connected
                                && matches!(&entry.source, EngineSource::Rich(state)
                                if refresh_due(state.checked_at, now))
                        })
                });
                match due {
                    Ok(true) => {
                        let _ = engine
                            .client()
                            .call(
                                methods::CHECK_ENGINE_UPDATE,
                                serde_json::json!({ "targetDeviceId": device }),
                            )
                            .await;
                    }
                    Ok(false) => {}
                    Err(_) => return,
                }
            }
        }
    });
    loop {
        let method = if rich {
            methods::WATCH_ENGINE_UPDATE
        } else {
            methods::UPDATE_STATUS
        };
        let result = engine
            .client()
            .subscribe_checked(method, serde_json::json!({ "targetDeviceId": device }))
            .await;
        match result {
            Ok(mut stream) => {
                while let Some(value) = stream.recv().await {
                    let source = if rich {
                        serde_json::from_value(value).map(EngineSource::Rich)
                    } else {
                        serde_json::from_value(value).map(EngineSource::Legacy)
                    };
                    let Ok(source) = source else {
                        break;
                    };
                    let alive = this.update(cx, |shell, cx| {
                        if let Some(entry) = shell.engine_update_devices.get_mut(&device) {
                            entry.source = source;
                            entry.connected = true;
                            entry.reconnecting_since = None;
                            // The engine's own account supersedes ours.
                            shell
                                .update_action_errors
                                .remove(&(device.clone(), Component::Engine));
                            cx.notify();
                        }
                    });
                    if alive.is_err() {
                        return;
                    }
                    retry = 1;
                }
            }
            Err(error) if rich && unsupported(&error) => {
                // A stale registry capability: fall back to the legacy probe.
                rich = false;
                continue;
            }
            // No such method, or an engine assembled without an updater:
            // both are permanent for that process, not worth retrying.
            Err(error) if unsupported(&error) => {
                this.update(cx, |shell, cx| {
                    if let Some(entry) = shell.engine_update_devices.get_mut(&device) {
                        entry.source = EngineSource::NoUpdateRpc;
                        entry.connected = true;
                        cx.notify();
                    }
                })
                .ok();
                return;
            }
            Err(_) => {}
        }
        let alive = this.update(cx, |shell, cx| {
            if let Some(entry) = shell.engine_update_devices.get_mut(&device) {
                entry.connected = false;
                let restarting = matches!(&entry.source, EngineSource::Rich(state)
                    if state.operation.as_ref().is_some_and(|op| op.phase == EnginePhase::Restarting));
                if restarting && entry.reconnecting_since.is_none() {
                    entry.reconnecting_since = Some(std::time::Instant::now());
                }
                cx.notify();
            }
        });
        if alive.is_err() {
            return;
        }
        cx.background_executor()
            .timer(Duration::from_secs(retry))
            .await;
        retry = (retry * 2).min(15);
    }
}

fn unsupported(error: &zeron_rpc::RpcError) -> bool {
    match error {
        zeron_rpc::RpcError::UnknownMethod(_) => true,
        zeron_rpc::RpcError::Failed(message) => {
            matches!(
                message.as_str(),
                "updates unavailable" | "engine updates unavailable"
            )
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::EngineUpdateOperation;

    fn state(running: &str, latest: &str, support: EngineUpdateSupport) -> EngineUpdateState {
        EngineUpdateState {
            running_version: running.into(),
            installed_version: Some(running.into()),
            latest_version: Some(latest.into()),
            checked_at: Some(1),
            check_error: None,
            support,
            operation: None,
        }
    }

    fn device(source: EngineSource) -> EngineDevice {
        EngineDevice {
            online: true,
            connected: true,
            rich: matches!(source, EngineSource::Rich(_)),
            source,
            ..Default::default()
        }
    }

    fn op(phase: EnginePhase) -> EngineUpdateOperation {
        EngineUpdateOperation {
            id: "op-1".into(),
            request_id: None,
            from_version: "0.2.90".into(),
            target_version: Some("0.2.97".into()),
            phase,
            started_at: 0,
            updated_at: 0,
            error: None,
        }
    }

    fn row(device: &EngineDevice) -> ZeronRow {
        engine_row(
            "demeter",
            "Demeter",
            false,
            device,
            std::time::Instant::now(),
            0,
        )
    }

    #[test]
    fn managed_remote_engines_offer_update_when_idle_with_device_identity() {
        let row = row(&device(EngineSource::Rich(state(
            "0.2.90",
            "0.2.97",
            EngineUpdateSupport::Managed,
        ))));
        assert_eq!(row.title, "Zeron");
        assert_eq!(row.device_name, "Demeter");
        assert_eq!(row.detail, "0.2.90 → 0.2.97");
        assert_eq!(row.action, Some(("Update when idle", RowAction::Start)));
        assert!(row.notice);
    }

    #[test]
    fn legacy_engines_explain_the_one_time_upgrade_instead_of_offering_apply() {
        // An old engine's ApplyUpdate restarts without waiting for idle, so
        // it is never exposed as a button.
        let row = row(&device(EngineSource::Legacy(zeron_update::UpdateStatus {
            current_version: "0.2.90".into(),
            latest_version: Some("0.2.97".into()),
            update_available: true,
            checked_at: Some(1),
            error: None,
        })));
        assert_eq!(
            row.action,
            Some(("View instructions", RowAction::Instructions))
        );
        assert!(row.explanation.unwrap().contains("`zeron update`"));
        assert!(row.notice);
    }

    #[test]
    fn engines_without_update_rpcs_are_a_state_not_a_retry() {
        let row = row(&device(EngineSource::NoUpdateRpc));
        assert_eq!(row.detail, "Can't report updates");
        assert!(!row.notice, "Settings-only: nothing is actionable remotely");
    }

    #[test]
    fn unsupported_and_unknown_support_never_offer_install() {
        for support in [
            EngineUpdateSupport::Unsupported {
                reason: "part of the desktop app".into(),
            },
            EngineUpdateSupport::Unknown,
        ] {
            let row = row(&device(EngineSource::Rich(state(
                "0.2.90", "0.2.97", support,
            ))));
            assert_eq!(
                row.action,
                Some(("View instructions", RowAction::Instructions))
            );
        }
    }

    #[test]
    fn a_restart_disconnect_is_reconnecting_then_an_honest_timeout() {
        let mut restarting = state("0.2.90", "0.2.97", EngineUpdateSupport::Managed);
        restarting.operation = Some(op(EnginePhase::Restarting));
        let mut offline = device(EngineSource::Rich(restarting));
        offline.connected = false;
        let started = std::time::Instant::now();
        offline.reconnecting_since = Some(started);
        let soon = engine_row("d", "Demeter", false, &offline, started, 0);
        assert_eq!(soon.tone, Tone::Active);
        assert!(soon.detail.starts_with("Reconnecting"));
        let late = engine_row(
            "d",
            "Demeter",
            false,
            &offline,
            started + RECONNECT_TIMEOUT + Duration::from_secs(1),
            0,
        );
        assert_eq!(late.tone, Tone::Danger);
        assert!(late.explanation.unwrap().contains("0.2.90"));
    }

    #[test]
    fn success_requires_the_running_version_and_restart_required_is_not_success() {
        let mut updated = state("0.2.97", "0.2.97", EngineUpdateSupport::Managed);
        updated.operation = Some(op(EnginePhase::Updated));
        let done = row(&device(EngineSource::Rich(updated)));
        assert_eq!(done.detail, "Updated to 0.2.97");
        assert_eq!(done.tone, Tone::Done);

        let mut pending = state("0.2.90", "0.2.97", EngineUpdateSupport::Managed);
        pending.installed_version = Some("0.2.97".into());
        pending.operation = Some(op(EnginePhase::RestartRequired));
        let pending = row(&device(EngineSource::Rich(pending)));
        assert_eq!(pending.detail, "0.2.97 installed · restart required");
        assert_eq!(
            pending.action,
            Some(("Restart required", RowAction::Instructions))
        );
        assert_ne!(pending.tone, Tone::Done);
    }

    #[test]
    fn a_restart_into_an_installed_version_is_not_described_as_an_install() {
        let mut waiting = state("0.2.90", "0.2.97", EngineUpdateSupport::Managed);
        waiting.operation = Some(op(EnginePhase::WaitingForIdle));
        assert!(
            row(&device(EngineSource::Rich(waiting.clone())))
                .detail
                .starts_with("0.2.97 installs when")
        );
        waiting.installed_version = Some("0.2.97".into());
        assert!(
            row(&device(EngineSource::Rich(waiting)))
                .detail
                .starts_with("Restarts into 0.2.97 when")
        );
    }

    #[test]
    fn unchecked_engine_is_not_reported_as_current() {
        let mut unchecked = state("0.2.97", "0.2.97", EngineUpdateSupport::Managed);
        unchecked.checked_at = None;
        unchecked.latest_version = None;
        let row = row(&device(EngineSource::Rich(unchecked)));
        assert!(row.detail.contains("not checked yet"));
        assert_eq!(row.action, Some(("Check for updates", RowAction::Check)));
    }

    #[test]
    fn only_pre_boundary_phases_offer_cancel() {
        for (phase, cancellable) in [
            (EnginePhase::Staging, true),
            (EnginePhase::WaitingForIdle, true),
            (EnginePhase::Applying, false),
            (EnginePhase::Restarting, false),
        ] {
            let mut s = state("0.2.90", "0.2.97", EngineUpdateSupport::Managed);
            s.operation = Some(op(phase));
            let row = row(&device(EngineSource::Rich(s)));
            assert_eq!(
                matches!(row.action, Some((_, RowAction::Cancel { .. }))),
                cancellable,
                "{phase:?}"
            );
            assert_eq!(row.tone, Tone::Active);
        }
    }

    #[test]
    fn offline_devices_keep_last_known_facts_without_live_actions() {
        let mut offline = device(EngineSource::Rich(state(
            "0.2.90",
            "0.2.97",
            EngineUpdateSupport::Managed,
        )));
        offline.online = false;
        offline.connected = false;
        let row = row(&offline);
        assert!(row.detail.starts_with("Offline · 0.2.90 → 0.2.97"));
        assert_eq!(row.action, None);
    }

    #[test]
    fn a_failed_download_can_be_retried_while_the_release_is_offered() {
        let mut failed = state("0.2.90", "0.2.97", EngineUpdateSupport::Managed);
        let mut operation = op(EnginePhase::Failed);
        operation.error = Some("could not stage 0.2.97: connection reset".into());
        failed.operation = Some(operation);
        let retry = row(&device(EngineSource::Rich(failed.clone())));
        assert_eq!(retry.action, Some(("Try again", RowAction::Start)));
        assert_eq!(retry.tone, Tone::Danger);
        // Nothing newer on offer any more: only a fresh check makes sense.
        failed.latest_version = Some("0.2.90".into());
        let retry = row(&device(EngineSource::Rich(failed)));
        assert_eq!(retry.action, Some(("Check again", RowAction::Check)));
    }

    #[test]
    fn presence_loss_during_a_restart_starts_the_reconnect_timeout() {
        let mut restarting = state("0.2.90", "0.2.97", EngineUpdateSupport::Managed);
        restarting.operation = Some(op(EnginePhase::Restarting));
        let mut devices = std::collections::BTreeMap::from([(
            "demeter".to_string(),
            device(EngineSource::Rich(restarting)),
        )]);
        devices.get_mut("demeter").unwrap().watch = Some(Task::ready(()));
        let desired = std::collections::BTreeMap::from([("demeter".to_string(), (false, true))]);
        reconcile_engines(&mut devices, &desired);
        let since = devices["demeter"]
            .reconnecting_since
            .expect("timer starts on presence loss");
        let late = engine_row(
            "demeter",
            "Demeter",
            false,
            &devices["demeter"],
            since + RECONNECT_TIMEOUT + Duration::from_secs(1),
            0,
        );
        assert_eq!(late.tone, Tone::Danger);
        assert_eq!(late.detail, "Didn't reconnect after restarting");
    }

    #[test]
    fn dismissal_hides_only_the_current_offer_and_never_progress_or_failure() {
        let offered = state("0.2.90", "0.2.97", EngineUpdateSupport::Managed);
        let row_of = |state: EngineUpdateState| row(&device(EngineSource::Rich(state)));
        let first = row_of(offered.clone());
        let dismissed = std::collections::HashSet::from([first.dismiss_key()]);
        assert!(first.hidden_by(&dismissed));
        // A newer release is a new offer.
        let newer = row_of(state("0.2.90", "0.2.98", EngineUpdateSupport::Managed));
        assert!(!newer.hidden_by(&dismissed));
        // Starting the install, or failing, brings the row back.
        let mut installing = offered.clone();
        installing.operation = Some(op(EnginePhase::Staging));
        assert!(!row_of(installing).hidden_by(&dismissed));
        let mut failed = offered;
        failed.operation = Some(op(EnginePhase::Failed));
        assert!(!row_of(failed).hidden_by(&dismissed));
    }

    #[test]
    fn a_failed_action_shows_on_its_own_row_and_keeps_the_action() {
        let offer = row(&device(EngineSource::Rich(state(
            "0.2.90",
            "0.2.97",
            EngineUpdateSupport::Managed,
        ))));
        let mut errors = ActionErrors::new();
        assert_eq!(offer.clone().with_action_error(&errors), offer);
        errors.insert(
            ("demeter".into(), Component::Engine),
            "0.2.97 is already installed; restart the engine to finish".into(),
        );
        let failed = offer.clone().with_action_error(&errors);
        assert_eq!(failed.detail, "Request failed");
        assert!(failed.explanation.unwrap().contains("already installed"));
        assert_eq!(failed.action, offer.action);
        assert!(failed.notice);
        // Never hidden by an earlier dismissal of the offer.
        let dismissed = [offer.dismiss_key()].into_iter().collect();
        assert!(offer.hidden_by(&dismissed));
        assert!(
            !offer
                .clone()
                .with_action_error(&errors)
                .hidden_by(&dismissed)
        );
        // Another device's or component's failure is not this row's.
        let mut other = ActionErrors::new();
        other.insert(("demeter".into(), Component::App), "busy".into());
        other.insert(("athena".into(), Component::Engine), "busy".into());
        assert_eq!(offer.clone().with_action_error(&other), offer);
    }

    #[test]
    fn stale_or_missing_check_times_trigger_a_refresh() {
        let now = 10_000_000_000;
        assert!(refresh_due(None, now));
        assert!(refresh_due(Some(now - 31 * 60 * 1000), now));
        assert!(!refresh_due(Some(now - 29 * 60 * 1000), now));
        // A clock ahead of the device never triggers a request storm.
        assert!(!refresh_due(Some(now + 60_000), now));
    }

    #[test]
    fn capability_upgrades_restart_only_that_devices_watch() {
        let mut devices = std::collections::BTreeMap::from([
            ("a".to_string(), device(EngineSource::Unknown)),
            ("b".to_string(), device(EngineSource::Unknown)),
        ]);
        for device in devices.values_mut() {
            device.rich = false;
            device.watch = Some(Task::ready(()));
        }
        let desired = std::collections::BTreeMap::from([
            ("a".to_string(), (true, true)),
            ("b".to_string(), (true, false)),
        ]);
        assert_eq!(reconcile_engines(&mut devices, &desired), ["a"]);
        assert!(devices["b"].watch.is_some());
        // Presence loss keeps the last state; departure removes the device.
        let desired = std::collections::BTreeMap::from([("a".to_string(), (false, true))]);
        assert!(reconcile_engines(&mut devices, &desired).is_empty());
        assert!(!devices.contains_key("b"));
        assert!(!devices["a"].connected);
    }
}
