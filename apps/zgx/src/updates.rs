//! What there is to update, on every device: each one's agents
//! (`WatchHarnessUpdates`, as the shipping app's home island watches them,
//! crates/ui/src/shell/harness_updates.rs) and, on another device, Zeron
//! itself (`UpdateStatus`; this Mac's is `about`'s: src/about.rs).
//!
//!   shared `updates`   { rows = { {
//!       id, device, device_name, here, connected, kind ("agent" | "zeron"),
//!       harness?, name?, mark?, phase, installed?, latest?, message?,
//!       action? ("update" | "steps" | "cancel" | "retry" | "check"), active,
//!   } } }
//!   action  `updates.run`  { device, harness, action }   an agent's
//!   action  `updates.apply_zeron`  { device }            another device's Zeron
//!
//! A device gone quiet keeps its last rows, marked not `connected`: its
//! actions wait for it. Zeron on another device updates from here when its
//! engine manages its own install (`ApplyUpdate`, a tarball's or the
//! installer's, on Linux); elsewhere the app there does (no `action`). One that
//! doesn't come back on a new version in time says so: a reconnect alone isn't
//! proof it updated.

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
    time::Duration,
};

use serde_json::{Value as Json, json};
use zeron_proto::{HarnessId, HarnessUpdateFailureKind, HarnessUpdatePhase as Phase, HarnessUpdateStatus};
use zgx_script::{ScriptHost, Shared, Value};

use crate::auth::text;
use crate::engine::{self, rpc::{Rpc, Watch}};

/// How long a Zeron being updated stays so without word from it: its engine
/// restarts, and says its new version once it's back.
const ZERON_UPDATE_PATIENCE: Duration = Duration::from_secs(180);
/// How long one that said it took the update stays so while it says nothing
/// more: its engine may take 10 minutes and a minute per GB of databases to
/// try the new version before it settles.
const ZERON_TRIAL_PATIENCE: Duration = Duration::from_secs(30 * 60);
const RETRY_MOST_SECS: u64 = 30;
/// A Zeron asked to update that hasn't come back on a new version.
const NOT_APPLIED: &str = "It didn't come back on the new version";
/// One whose new version failed, when it doesn't say why.
const WENT_BACK: &str = "The new version didn't start, so it went back";
/// One that never got as far as the new version, when it doesn't say why.
const DIDNT_UPDATE: &str = "It couldn't update";

/// The latest update asked of a Zeron, as `UpdateStatus` reports it. (From
/// an engine too old to report, nothing: its version is all there is to go by.)
#[derive(Clone, PartialEq)]
struct Report {
    id: String,
    from: String,
    state: String,
    reason: Option<String>,
}

impl Report {
    fn of(item: &Json) -> Option<Report> {
        let report = &item["update"];
        Some(Report {
            id: report["id"].as_str()?.to_owned(),
            from: report["fromVersion"].as_str().unwrap_or_default().to_owned(),
            state: report["state"].as_str().unwrap_or_default().to_owned(),
            reason: report["reason"].as_str().filter(|r| !r.trim().is_empty()).map(Into::into),
        })
    }
}

/// Zeron on another device, as `UpdateStatus` says.
#[derive(Clone, Default, PartialEq)]
struct Zeron {
    current: String,
    latest: Option<String>,
    available: bool,
    /// Whether it says it can update from here (nothing: an engine too old
    /// to say), why not in its words, and what to run there instead.
    can_apply: Option<bool>,
    why_not: Option<String>,
    manual_command: Option<String>,
    report: Option<Report>,
}

#[derive(Default)]
struct Device {
    name: String,
    platform: String,
    /// It reports its agents' updates (the shipping app's capability check).
    reports_agents: bool,
    online: bool,
    connected: bool,
    statuses: Vec<HarnessUpdateStatus>,
    /// Why an agent's update waits, in the engine's words, by harness.
    wait_reasons: BTreeMap<String, String>,
    zeron: Option<Zeron>,
    /// The version it was on as its Zeron was asked to update.
    updating_from: Option<String>,
    /// The update its Zeron said it took, to hear how that one went.
    update_id: Option<String>,
    /// Why its Zeron didn't update.
    zeron_error: Option<String>,
    retries: u64,
    watches: Vec<Watch>,
}

#[derive(Default)]
struct Updates {
    shared: RefCell<Vec<Shared>>,
    devices: RefCell<BTreeMap<String, Device>>,
    here: RefCell<Option<String>>,
    rpc: RefCell<Option<Rpc>>,
    /// Bumped with each connection: a retry from an old one does nothing.
    connection: Cell<u64>,
    list: RefCell<Option<crate::engine::lists::Listening>>,
    /// Publishes so far (for tests).
    #[cfg(test)]
    published: Cell<u32>,
}

thread_local! {
    static UPDATES: Rc<Updates> = Rc::new(Updates::default());
}

pub fn install(host: &ScriptHost) {
    let updates = UPDATES.with(Rc::clone);
    updates.shared.borrow_mut().push(host.shared("updates", updates.value()));
    if updates.shared.borrow().len() == 1 {
        let weak = Rc::downgrade(&updates);
        engine::get().on_connect(move |rpc| {
            if let Some(updates) = weak.upgrade() {
                updates.connected(rpc);
            }
        });
    }
    let a = updates.clone();
    host.async_action("updates.run", move |args| {
        let a = a.clone();
        async move { a.run(args).await }
    });
    let a = updates;
    host.async_action("updates.apply_zeron", move |args| {
        let a = a.clone();
        async move { a.apply_zeron(args).await }
    });
}

impl Updates {
    fn connected(self: &Rc<Self>, rpc: &Rpc) {
        self.connection.set(self.connection.get() + 1);
        *self.rpc.borrow_mut() = Some(rpc.clone());
        for device in self.devices.borrow_mut().values_mut() {
            device.watches.clear();
            device.connected = false;
            device.retries = 0;
        }
        let weak = Rc::downgrade(self);
        let list = crate::engine::lists::listen(rpc, "WatchDevices", move |frame| {
            if let Some(updates) = weak.upgrade() {
                updates.heard_devices(frame.list);
            }
        });
        *self.list.borrow_mut() = Some(list);
        let (me, rpc) = (self.clone(), rpc.clone());
        engine::get().spawn(async move {
            if let Some(here) = engine::local_device(&rpc).await {
                *me.here.borrow_mut() = Some(here);
                me.publish();
            }
        });
    }

    fn heard_devices(self: &Rc<Self>, list: &Json) {
        let now = crate::sessions::now_seconds();
        let here = self.here.borrow().clone();
        let mut start = Vec::new();
        // Whether anything the rows are made from moved: the list comes again
        // with every device's heartbeat, which moves none of it.
        let mut changed = false;
        {
            let mut devices = self.devices.borrow_mut();
            let wanted: Vec<&Json> = list
                .as_array()
                .into_iter()
                .flatten()
                .filter(|d| !crate::sessions::is_cloud(d) && d["id"].as_str().is_some())
                .collect();
            let before = devices.len();
            devices.retain(|id, _| wanted.iter().any(|d| d["id"].as_str() == Some(id)));
            changed |= devices.len() != before;
            for d in wanted {
                let id = d["id"].as_str().unwrap_or_default();
                let device = devices.entry(id.to_owned()).or_insert_with(|| {
                    changed = true;
                    Device::default()
                });
                let was = (device.name.clone(), device.platform.clone(), device.reports_agents, device.online, device.connected);
                device.name = d["name"].as_str().unwrap_or("Device").to_owned();
                device.platform = d["platform"].as_str().unwrap_or_default().to_owned();
                device.reports_agents = here.as_deref() == Some(id)
                    || d["capabilities"].as_array().is_some_and(|c| {
                        c.iter().any(|c| c.as_str() == Some(zeron_proto::capabilities::HARNESS_UPDATES_V1))
                    });
                let seen = d["lastSeenAt"].as_str().and_then(crate::sessions::unix_seconds);
                device.online = here.as_deref() == Some(id)
                    || seen.is_some_and(|at| now - at <= crate::sessions::ONLINE_SECS);
                if !device.online {
                    device.watches.clear();
                    device.connected = false;
                } else if device.watches.is_empty() {
                    start.push(id.to_owned());
                }
                changed |= was != (device.name.clone(), device.platform.clone(), device.reports_agents, device.online, device.connected);
            }
        }
        for id in start {
            self.watch_device(&id);
        }
        if changed {
            self.publish();
        }
    }

    /// `params` for a call to `device`: this Mac's engine takes them as they are.
    fn addressed(&self, device: &str, mut params: Json) -> Json {
        if self.here.borrow().as_deref() != Some(device)
            && let Some(object) = params.as_object_mut()
        {
            object.insert("targetDeviceId".into(), json!(device));
        }
        params
    }

    fn watch_device(self: &Rc<Self>, id: &str) {
        let Some(rpc) = self.rpc.borrow().clone() else { return };
        let connection = self.connection.get();
        let mut watches = Vec::new();
        let reports_agents = self.devices.borrow().get(id).is_some_and(|d| d.reports_agents);
        if reports_agents {
            let (weak, device) = (Rc::downgrade(self), id.to_owned());
            let (ended, gone) = (weak.clone(), device.clone());
            watches.push(rpc.watch_ending(
                "WatchHarnessUpdates",
                self.addressed(id, json!({})),
                move |item| {
                    let Some(me) = weak.upgrade() else { return };
                    let wait_reasons = wait_reasons_of(&item);
                    let Ok(statuses) = serde_json::from_value::<Vec<HarnessUpdateStatus>>(item) else { return };
                    if let Some(d) = me.devices.borrow_mut().get_mut(&device) {
                        d.statuses = statuses;
                        d.wait_reasons = wait_reasons;
                        d.connected = true;
                        d.retries = 0;
                    }
                    me.publish();
                },
                move |why| {
                    if let Some(me) = ended.upgrade() {
                        me.ended(&gone, connection, &why);
                    }
                },
            ));
        }
        // (This Mac's Zeron is the app's own: `about`.)
        if self.here.borrow().as_deref() != Some(id) {
            // (Its stream ends as its engine restarts to update: watched again
            // from then on, or the new version would never be heard.)
            let (weak, device) = (Rc::downgrade(self), id.to_owned());
            let (ended, gone) = (weak.clone(), device.clone());
            watches.push(rpc.watch_ending(
                "UpdateStatus",
                self.addressed(id, json!({})),
                move |item| {
                    if let Some(me) = weak.upgrade() {
                        me.heard_zeron(&device, &item);
                    }
                },
                move |why| {
                    if let Some(me) = ended.upgrade() {
                        me.ended(&gone, connection, &why);
                    }
                },
            ));
        }
        if let Some(d) = self.devices.borrow_mut().get_mut(id) {
            d.watches = watches;
        }
    }

    /// A device's watch ended, the connection still up: tried again, a little
    /// later each time, while it's online.
    fn ended(self: &Rc<Self>, id: &str, connection: u64, why: &str) {
        let wait = {
            let mut devices = self.devices.borrow_mut();
            let Some(d) = devices.get_mut(id) else { return };
            d.connected = false;
            d.watches.clear();
            d.retries += 1;
            eprintln!("zeron-zgx: updates: {} stopped answering: {why}", d.name);
            (1u64 << d.retries.min(5)).min(RETRY_MOST_SECS)
        };
        self.publish();
        let (me, id) = (self.clone(), id.to_owned());
        engine::get().spawn(async move {
            zgx::timer::sleep(Duration::from_secs(wait)).await;
            let again = me.connection.get() == connection
                && me.devices.borrow().get(&id).is_some_and(|d| d.online && d.watches.is_empty());
            if again {
                me.watch_device(&id);
            }
        });
    }

    fn heard_zeron(&self, id: &str, item: &Json) {
        let usable = item["held"].is_null() && !item["switch"].as_bool().unwrap_or(false);
        let zeron = Zeron {
            // (A beta's app, which isn't its engine: `currentVersion` is the engine's.)
            current: item["appVersion"].as_str().or(item["currentVersion"].as_str()).unwrap_or_default().to_owned(),
            latest: item["latestVersion"].as_str().map(Into::into),
            available: usable && item["updateAvailable"].as_bool().unwrap_or(false),
            can_apply: item["canApplyRemotely"].as_bool(),
            why_not: item["cannotApplyReason"].as_str().map(Into::into),
            manual_command: item["manualCommand"].as_str().map(Into::into),
            report: Report::of(item),
        };
        if let Some(d) = self.devices.borrow_mut().get_mut(id) {
            d.zeron = Some(zeron);
            d.connected = true;
            d.settle();
        }
        self.publish();
    }

    async fn run(&self, args: Value) -> Result<Value, String> {
        let field = |key: &str| args.field(key).map(text).unwrap_or_default();
        let device = field("device");
        let method = method_of(&field("action")).ok_or("unknown")?;
        let params = self.addressed(&device, json!({ "harness": field("harness") }));
        // (Its progress comes by its watch.)
        match engine::get().call(method, params).await {
            Ok(_) => Ok(Value::Nil),
            Err(error) => {
                let error = error.to_string();
                eprintln!("zeron-zgx: updates: {method} on {device}: {error}");
                Err(error)
            }
        }
    }

    async fn apply_zeron(self: Rc<Self>, args: Value) -> Result<Value, String> {
        let device = args.field("device").map(text).unwrap_or_default();
        let from = {
            let mut devices = self.devices.borrow_mut();
            let d = devices.get_mut(&device).ok_or("unknown")?;
            let from = d.zeron.as_ref().map(|z| z.current.clone()).ok_or("unknown")?;
            d.updating_from = Some(from.clone());
            d.zeron_error = None;
            from
        };
        self.publish();
        let params = self.addressed(&device, json!({}));
        let answer = match engine::get().call("ApplyUpdate", params).await {
            Ok(answer) => answer,
            Err(error) => {
                let error = error.to_string();
                eprintln!("zeron-zgx: updates: Zeron on {device}: {error}");
                if let Some(d) = self.devices.borrow_mut().get_mut(&device) {
                    d.updating_from = None;
                    d.update_id = None;
                    d.zeron_error = Some(error.clone());
                }
                self.publish();
                return Err(error);
            }
        };
        if let Some(d) = self.devices.borrow_mut().get_mut(&device) {
            if let Some(id) = answer["updateId"].as_str() {
                d.update_id = Some(id.to_owned());
            }
            // (How it went may have come first.)
            d.settle();
        }
        self.publish();
        // (Its engine restarts now: the new version comes as it's back. One
        // that never comes shows its update again.)
        let me = self.clone();
        engine::get().spawn(async move {
            zgx::timer::sleep(ZERON_UPDATE_PATIENCE).await;
            let tracked = me.devices.borrow().get(&device).is_some_and(|d| d.update_id.is_some());
            if tracked {
                zgx::timer::sleep(ZERON_TRIAL_PATIENCE.saturating_sub(ZERON_UPDATE_PATIENCE)).await;
            }
            let late = me
                .devices
                .borrow_mut()
                .get_mut(&device)
                .filter(|d| d.updating_from.as_deref() == Some(from.as_str()))
                .map(|d| {
                    d.updating_from = None;
                    d.update_id = None;
                    d.zeron_error = Some(NOT_APPLIED.into());
                })
                .is_some();
            if late {
                me.publish();
            }
        });
        Ok(Value::Nil)
    }

    fn publish(&self) {
        #[cfg(test)]
        self.published.set(self.published.get() + 1);
        let value = self.value();
        for shared in self.shared.borrow().iter() {
            shared.set(value.clone());
        }
    }

    fn value(&self) -> Value {
        let here = self.here.borrow();
        let devices = self.devices.borrow();
        // This Mac's first, then by name.
        let mut order: Vec<(&String, &Device)> = devices.iter().collect();
        order.sort_by_key(|(id, d)| (Some(id.as_str()) != here.as_deref(), d.name.clone()));
        let mut rows = Vec::new();
        // (This Mac's Zeron is the app's own, whatever its engine says.)
        for (id, d) in &order {
            if Some(id.as_str()) != here.as_deref() {
                rows.extend(zeron_row(id, d));
            }
        }
        for (id, d) in &order {
            let is_here = Some(id.as_str()) == here.as_deref();
            rows.extend(d.statuses.iter().filter(|s| s.show_update_notice()).map(|s| agent_row(id, d, is_here, s)));
        }
        Value::record([("rows", Value::List(rows.into()))])
    }
}

impl Device {
    /// What's known now of the update its Zeron was asked for. An engine that
    /// reports says how it went, as reconnecting doesn't; an older one is
    /// done once its version moves.
    fn settle(&mut self) {
        let Some(zeron) = &self.zeron else { return };
        let report = zeron.report.as_ref();
        // Under way, though not asked here or its answer never came (the app
        // restarted, or the engine went before replying).
        if self.update_id.is_none()
            && let Some(report) = report.filter(|r| r.state == "inProgress")
        {
            self.update_id = Some(report.id.clone());
            self.updating_from.get_or_insert_with(|| report.from.clone());
            self.zeron_error = None;
        }
        let Some(id) = &self.update_id else {
            if self.updating_from.as_deref().is_some_and(|from| from != zeron.current) {
                self.updating_from = None;
                self.zeron_error = None;
            }
            return;
        };
        let Some(report) = report.filter(|r| &r.id == id) else { return };
        let error = match report.state.as_str() {
            "updated" => None,
            "rolledBack" => Some(report.reason.clone().unwrap_or_else(|| WENT_BACK.into())),
            "failed" => Some(report.reason.clone().unwrap_or_else(|| DIDNT_UPDATE.into())),
            // Under way, or a state this app doesn't know yet.
            _ => return,
        };
        self.updating_from = None;
        self.update_id = None;
        self.zeron_error = error;
    }
}

/// Why agents' updates wait, by harness, from `WatchHarnessUpdates`. (Absent
/// from older engines, whose rows keep their own word.)
fn wait_reasons_of(item: &Json) -> BTreeMap<String, String> {
    item.as_array()
        .into_iter()
        .flatten()
        .filter_map(|status| {
            let reason = status["waitReason"].as_str().filter(|r| !r.trim().is_empty())?;
            Some((status["harness"].as_str()?.to_owned(), reason.to_owned()))
        })
        .collect()
}

fn method_of(action: &str) -> Option<&'static str> {
    Some(match action {
        "update" | "retry" => "ApplyHarnessUpdate",
        "cancel" => "CancelHarnessUpdate",
        "check" => "CheckHarnessUpdates",
        _ => return None,
    })
}

/// Another device's Zeron with an update, or being updated.
fn zeron_row(id: &str, d: &Device) -> Option<Value> {
    let zeron = d.zeron.as_ref()?;
    let updating = d.updating_from.is_some();
    if !updating && !zeron.available && d.zeron_error.is_none() {
        return None;
    }
    // As it says; an engine too old to say is tried where it can be (a Linux
    // service can, and says why not if it can't).
    let can_apply = zeron.can_apply.unwrap_or(d.platform == "linux");
    let phase = if updating {
        "installing"
    } else if d.zeron_error.is_some() {
        "failed"
    } else {
        "available"
    };
    let mut fields = vec![
        ("id", Value::from(format!("{id}:zeron").as_str())),
        ("device", Value::from(id)),
        ("device_name", Value::from(d.name.as_str())),
        ("here", Value::Bool(false)),
        ("connected", Value::Bool(d.online && d.connected)),
        ("kind", Value::from("zeron")),
        ("phase", Value::from(phase)),
        ("installed", Value::from(zeron.current.as_str())),
        ("active", Value::Bool(updating)),
    ];
    if let Some(latest) = &zeron.latest {
        fields.push(("latest", Value::from(latest.as_str())));
    }
    if let Some(error) = &d.zeron_error {
        fields.push(("message", Value::from(error.as_str())));
    }
    if !updating && can_apply {
        fields.push(("action", Value::from(if d.zeron_error.is_some() { "retry" } else { "update" })));
    }
    if !updating && !can_apply {
        if let Some(why) = &zeron.why_not {
            fields.push(("why_not", Value::from(why.as_str())));
        }
        if let Some(command) = &zeron.manual_command {
            fields.push(("manual_command", Value::from(command.as_str())));
            fields.push(("action", Value::from("copy")));
        }
    }
    Some(Value::record(fields))
}

fn agent_row(id: &str, d: &Device, here: bool, status: &HarnessUpdateStatus) -> Value {
    let harness = harness_wire(&status.harness);
    let mut fields = vec![
        ("id", Value::from(format!("{id}:{harness}").as_str())),
        ("device", Value::from(id)),
        ("device_name", Value::from(d.name.as_str())),
        ("here", Value::Bool(here)),
        ("connected", Value::Bool(d.online && d.connected)),
        ("kind", Value::from("agent")),
        ("harness", Value::from(harness.as_str())),
        ("name", Value::from(agent_name(&status.harness))),
        ("phase", Value::from(phase_word(status.phase))),
        ("active", Value::Bool(active(status))),
    ];
    if let Some(mark) = crate::agents::glyph(&harness) {
        fields.push(("mark", Value::from(mark.as_str())));
    }
    if let Some(icon) = crate::agents::registry_icon(&harness) {
        fields.push(("agent_icon", Value::from(icon.as_str())));
    }
    if let Some(v) = &status.installed_version {
        fields.push(("installed", Value::from(v.as_str())));
    }
    if let Some(v) = &status.latest_version {
        fields.push(("latest", Value::from(v.as_str())));
    }
    let waiting = (status.phase == Phase::WaitingForIdle).then(|| d.wait_reasons.get(&harness).cloned()).flatten();
    if let Some(message) = waiting.or_else(|| message_of(status)) {
        fields.push(("message", Value::from(message.as_str())));
    }
    if let Some(action) = action_of(status) {
        fields.push(("action", Value::from(action)));
    }
    Value::record(fields)
}

fn harness_wire(harness: &HarnessId) -> String {
    serde_json::to_value(harness).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

/// An agent's name, as the shipping app's list says it.
fn agent_name(harness: &HarnessId) -> &str {
    match harness {
        HarnessId::ClaudeCode => "Claude Code",
        HarnessId::Codex => "Codex",
        HarnessId::Cursor => "Cursor",
        HarnessId::Devin => "Devin",
        HarnessId::Grok => "Grok",
        HarnessId::Hermes => "Hermes",
        HarnessId::Fx => "fx",
        HarnessId::Pi => "Pi",
        HarnessId::Opencode => "OpenCode",
        HarnessId::Antigravity => "Antigravity",
        HarnessId::Muse => "Muse Code",
        HarnessId::Mock => "Mock",
        HarnessId::Registry(agent) => agent.registry_id(),
    }
}

fn phase_word(phase: Phase) -> &'static str {
    match phase {
        Phase::Available => "available",
        Phase::WaitingForIdle => "waiting",
        Phase::Preparing => "preparing",
        Phase::Downloading => "downloading",
        Phase::Installing => "installing",
        Phase::Verifying => "verifying",
        Phase::Updated => "updated",
        Phase::Failed => "failed",
        _ => "checking",
    }
}

fn active(status: &HarnessUpdateStatus) -> bool {
    matches!(
        status.phase,
        Phase::WaitingForIdle | Phase::Preparing | Phase::Downloading | Phase::Installing | Phase::Verifying
    )
}

/// The update itself failed (not a check): its release is still known.
fn install_failed(status: &HarnessUpdateStatus) -> bool {
    status.error.as_ref().is_some_and(|error| error.kind == HarnessUpdateFailureKind::Install)
}

/// What its row offers, as the shipping app's (`action`).
fn action_of(status: &HarnessUpdateStatus) -> Option<&'static str> {
    match status.phase {
        Phase::Available if status.can_apply => Some("update"),
        Phase::Available => Some("steps"),
        Phase::WaitingForIdle | Phase::Preparing | Phase::Downloading => Some("cancel"),
        Phase::Failed if status.can_apply && install_failed(status) => Some("retry"),
        Phase::Failed => Some("check"),
        _ => None,
    }
}

/// What the engine says of it in its own words: a download's progress, why
/// it failed.
fn message_of(status: &HarnessUpdateStatus) -> Option<String> {
    match status.phase {
        Phase::Downloading => status.progress.as_ref().and_then(|p| p.message.clone()),
        Phase::Failed => status.error.as_ref().map(|e| e.message.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(phase: &str, can_apply: bool) -> HarnessUpdateStatus {
        serde_json::from_value(json!({
            "harness": "codex", "phase": phase, "can_apply": can_apply,
            "canApply": can_apply, "installedVersion": "1.0", "latestVersion": "1.1",
            "installed_version": "1.0", "latest_version": "1.1",
        }))
        .unwrap()
    }

    /// The device list comes again with every device's heartbeat: a beat
    /// that leaves each device as it was (online still) publishes nothing; a
    /// device renamed, gone quiet or gone does.
    #[test]
    fn a_device_s_heartbeat_publishes_nothing() {
        let updates = Rc::new(Updates::default());
        *updates.here.borrow_mut() = Some("mac".into());
        let now = crate::sessions::now_seconds();
        let at = crate::sessions::rfc3339;
        let list = |box_name: &str, box_seen: i64| json!([
            { "id": "mac", "name": "Mac", "platform": "macos", "lastSeenAt": at(now) },
            { "id": "box", "name": box_name, "platform": "linux", "lastSeenAt": at(box_seen) },
        ]);
        updates.heard_devices(&list("Box", now - 3));
        let first = updates.published.get();
        assert_eq!(first, 1);
        updates.heard_devices(&list("Box", now));
        updates.heard_devices(&list("Box", now));
        assert_eq!(updates.published.get(), first, "a beat moves nothing");
        updates.heard_devices(&list("Build box", now));
        assert_eq!(updates.published.get(), first + 1);
        updates.heard_devices(&list("Build box", now - 600));
        assert_eq!(updates.published.get(), first + 2, "gone quiet");
        assert!(!updates.devices.borrow()["box"].online);
        updates.heard_devices(&json!([{ "id": "mac", "name": "Mac", "platform": "macos", "lastSeenAt": at(now) }]));
        assert_eq!(updates.published.get(), first + 3, "gone");
    }

    #[test]
    fn a_row_offers_what_the_shipping_app_s_does() {
        assert_eq!(action_of(&status("available", true)), Some("update"));
        assert_eq!(action_of(&status("available", false)), Some("steps"));
        assert_eq!(action_of(&status("downloading", true)), Some("cancel"));
        assert_eq!(action_of(&status("installing", true)), None);
        assert_eq!(action_of(&status("failed", false)), Some("check"));
        assert!(active(&status("verifying", true)));
        assert!(!active(&status("available", true)));
    }

    #[test]
    fn another_device_s_zeron_shows_only_with_an_update_and_updates_on_linux() {
        let mut d = Device { name: "metal".into(), platform: "linux".into(), online: true, connected: true, ..Device::default() };
        assert!(zeron_row("m", &d).is_none());
        d.zeron = Some(Zeron { current: "1".into(), latest: Some("2".into()), ..Zeron::default() });
        assert!(zeron_row("m", &d).is_none());
        d.zeron.as_mut().unwrap().available = true;
        let row = zeron_row("m", &d).unwrap();
        assert_eq!(row.field("action").map(text).as_deref(), Some("update"));
        assert_eq!(row.field("latest").map(text).as_deref(), Some("2"));
        // A Mac's or a PC's app updates itself, there.
        d.platform = "macos".into();
        assert!(zeron_row("m", &d).unwrap().field("action").is_none());
        // What it says of itself comes first.
        d.zeron.as_mut().unwrap().can_apply = Some(true);
        assert_eq!(zeron_row("m", &d).unwrap().field("action").map(text).as_deref(), Some("update"));
        d.platform = "linux".into();
        let z = d.zeron.as_mut().unwrap();
        z.can_apply = Some(false);
        z.why_not = Some("It was started by hand".into());
        z.manual_command = Some("zeron update".into());
        let row = zeron_row("m", &d).unwrap();
        assert_eq!(row.field("action").map(text).as_deref(), Some("copy"));
        assert_eq!(row.field("why_not").map(text).as_deref(), Some("It was started by hand"));
        d.zeron.as_mut().unwrap().can_apply = None;
        // Being updated: nothing to press.
        d.platform = "linux".into();
        d.updating_from = Some("1".into());
        let row = zeron_row("m", &d).unwrap();
        assert!(row.field("action").is_none());
        assert_eq!(row.field("phase").map(text).as_deref(), Some("installing"));
    }

    #[test]
    fn a_held_or_channel_leaving_release_isn_t_offered() {
        let updates = Updates::default();
        updates.devices.borrow_mut().insert("m".into(), Device::default());
        updates.heard_zeron("m", &json!({ "currentVersion": "1", "latestVersion": "2", "updateAvailable": true, "held": "older macOS" }));
        assert!(!updates.devices.borrow()["m"].zeron.as_ref().unwrap().available);
        updates.heard_zeron("m", &json!({ "currentVersion": "1", "latestVersion": "2", "updateAvailable": true, "switch": true }));
        assert!(!updates.devices.borrow()["m"].zeron.as_ref().unwrap().available);
        updates.heard_zeron("m", &json!({ "currentVersion": "1", "latestVersion": "2", "updateAvailable": true }));
        assert!(updates.devices.borrow()["m"].zeron.as_ref().unwrap().available);
    }

    fn asked(from: &str) -> Updates {
        let updates = Updates::default();
        let device = Device {
            name: "metal".into(),
            online: true,
            connected: true,
            updating_from: Some(from.into()),
            update_id: Some("u1".into()),
            ..Device::default()
        };
        updates.devices.borrow_mut().insert("m".into(), device);
        updates
    }

    fn heard(updates: &Updates, current: &str, id: &str, state: &str, reason: Option<&str>) {
        updates.heard_zeron(
            "m",
            &json!({ "currentVersion": current, "latestVersion": "2",
                     "update": { "id": id, "fromVersion": "1", "targetVersion": "2", "state": state, "reason": reason } }),
        );
    }

    #[test]
    fn a_zeron_s_update_ends_as_its_engine_says_not_as_it_reconnects() {
        let updates = asked("1");
        let device = |f: &dyn Fn(&Device)| f(&updates.devices.borrow()["m"]);
        // An older update's word says nothing of this one.
        heard(&updates, "1", "older", "rolledBack", Some("old news"));
        device(&|d| assert!(d.updating_from.is_some() && d.zeron_error.is_none()));
        // Back from a restart, on the new version, but still on trial.
        heard(&updates, "2", "u1", "inProgress", None);
        device(&|d| assert!(d.updating_from.is_some()));
        // A state this app doesn't know yet changes nothing.
        heard(&updates, "2", "u1", "someNewState", None);
        device(&|d| assert!(d.updating_from.is_some()));
        heard(&updates, "2", "u1", "updated", None);
        device(&|d| assert!(d.updating_from.is_none() && d.update_id.is_none() && d.zeron_error.is_none()));
    }

    #[test]
    fn a_zeron_that_went_back_says_so_and_offers_it_again() {
        let updates = asked("1");
        heard(&updates, "1", "u1", "rolledBack", Some("The new version crashed"));
        {
            let devices = updates.devices.borrow();
            assert_eq!(devices["m"].zeron_error.as_deref(), Some("The new version crashed"));
            assert!(devices["m"].updating_from.is_none());
        }
        let mut devices = updates.devices.borrow_mut();
        let d = devices.get_mut("m").unwrap();
        d.zeron.as_mut().unwrap().can_apply = Some(true);
        let row = zeron_row("m", d).unwrap();
        assert_eq!(row.field("phase").map(text).as_deref(), Some("failed"));
        assert_eq!(row.field("action").map(text).as_deref(), Some("retry"));
        assert_eq!(row.field("message").map(text).as_deref(), Some("The new version crashed"));
        drop(devices);
        // Without a reason, plain words.
        let updates = asked("1");
        heard(&updates, "1", "u1", "failed", None);
        assert_eq!(updates.devices.borrow()["m"].zeron_error.as_deref(), Some(DIDNT_UPDATE));
    }

    #[test]
    fn an_update_under_way_is_picked_up_after_a_restart_of_this_app() {
        let updates = Updates::default();
        updates.devices.borrow_mut().insert("m".into(), Device::default());
        heard(&updates, "1", "u9", "inProgress", None);
        {
            let devices = updates.devices.borrow();
            assert_eq!(devices["m"].update_id.as_deref(), Some("u9"));
            assert_eq!(devices["m"].updating_from.as_deref(), Some("1"));
        }
        heard(&updates, "1", "u9", "rolledBack", None);
        assert_eq!(updates.devices.borrow()["m"].zeron_error.as_deref(), Some(WENT_BACK));
        // An update settled long ago, heard fresh, isn't news.
        let updates = Updates::default();
        updates.devices.borrow_mut().insert("m".into(), Device::default());
        heard(&updates, "1", "u0", "rolledBack", Some("old news"));
        assert!(updates.devices.borrow()["m"].zeron_error.is_none());
    }

    #[test]
    fn an_engine_too_old_to_report_is_done_when_its_version_moves() {
        let updates = asked("1");
        updates.devices.borrow_mut().get_mut("m").unwrap().update_id = None;
        updates.heard_zeron("m", &json!({ "currentVersion": "1" }));
        assert!(updates.devices.borrow()["m"].updating_from.is_some());
        updates.heard_zeron("m", &json!({ "currentVersion": "2" }));
        assert!(updates.devices.borrow()["m"].updating_from.is_none());
    }

    #[test]
    fn a_beta_s_zeron_goes_by_its_app_version_not_its_engine_s() {
        let updates = Updates::default();
        updates.devices.borrow_mut().insert("m".into(), Device::default());
        updates.heard_zeron("m", &json!({ "currentVersion": "0.2.136", "appVersion": "0.2.137-beta.17" }));
        assert_eq!(updates.devices.borrow()["m"].zeron.as_ref().unwrap().current, "0.2.137-beta.17");
        updates.heard_zeron("m", &json!({ "currentVersion": "0.2.136" }));
        assert_eq!(updates.devices.borrow()["m"].zeron.as_ref().unwrap().current, "0.2.136");
    }

    #[test]
    fn an_agent_waiting_says_why_in_the_engine_s_words() {
        let item = json!([{ "harness": "codex", "phase": "waitingForIdle",
                            "waitReason": "Waiting for running chats to finish." },
                          { "harness": "grok", "phase": "available" }]);
        let reasons = wait_reasons_of(&item);
        assert_eq!(reasons.get("codex").map(String::as_str), Some("Waiting for running chats to finish."));
        assert_eq!(reasons.len(), 1);
        let mut d = Device { name: "metal".into(), wait_reasons: reasons, ..Device::default() };
        let mut waiting = status("available", true);
        waiting.phase = Phase::WaitingForIdle;
        assert_eq!(agent_row("m", &d, false, &waiting).field("message").map(text).as_deref(),
                   Some("Waiting for running chats to finish."));
        // An older engine's: none, so the row keeps its own word.
        d.wait_reasons.clear();
        assert!(agent_row("m", &d, false, &waiting).field("message").is_none());
    }
}
