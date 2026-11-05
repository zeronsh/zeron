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
//! installer's, on Linux); elsewhere the app there does (`can_apply` false).

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
const RETRY_MOST_SECS: u64 = 30;

/// Zeron on another device, as `UpdateStatus` says.
#[derive(Clone, Default, PartialEq)]
struct Zeron {
    current: String,
    latest: Option<String>,
    available: bool,
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
    zeron: Option<Zeron>,
    /// The version it was on as its Zeron was asked to update.
    updating_from: Option<String>,
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
    list: RefCell<Option<Watch>>,
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
        let list = rpc.watch("WatchDevices", json!({}), move |item| {
            if let Some(updates) = weak.upgrade() {
                updates.heard_devices(&item);
            }
        });
        *self.list.borrow_mut() = Some(list);
        let (me, rpc) = (self.clone(), rpc.clone());
        engine::get().spawn(async move {
            if let Ok(answer) = rpc.call("LocalDevice", json!({})).await {
                *me.here.borrow_mut() = answer["deviceId"].as_str().map(Into::into);
                me.publish();
            }
        });
    }

    fn heard_devices(self: &Rc<Self>, list: &Json) {
        let now = crate::sessions::now_seconds();
        let here = self.here.borrow().clone();
        let mut start = Vec::new();
        {
            let mut devices = self.devices.borrow_mut();
            let wanted: Vec<&Json> = list
                .as_array()
                .into_iter()
                .flatten()
                .filter(|d| !crate::sessions::is_cloud(d) && d["id"].as_str().is_some())
                .collect();
            devices.retain(|id, _| wanted.iter().any(|d| d["id"].as_str() == Some(id)));
            for d in wanted {
                let id = d["id"].as_str().unwrap_or_default();
                let device = devices.entry(id.to_owned()).or_default();
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
            }
        }
        for id in start {
            self.watch_device(&id);
        }
        self.publish();
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
                    let Ok(statuses) = serde_json::from_value::<Vec<HarnessUpdateStatus>>(item) else { return };
                    if let Some(d) = me.devices.borrow_mut().get_mut(&device) {
                        d.statuses = statuses;
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
            let (weak, device) = (Rc::downgrade(self), id.to_owned());
            watches.push(rpc.watch("UpdateStatus", self.addressed(id, json!({})), move |item| {
                if let Some(me) = weak.upgrade() {
                    me.heard_zeron(&device, &item);
                }
            }));
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
            current: item["currentVersion"].as_str().unwrap_or_default().to_owned(),
            latest: item["latestVersion"].as_str().map(Into::into),
            available: usable && item["updateAvailable"].as_bool().unwrap_or(false),
        };
        if let Some(d) = self.devices.borrow_mut().get_mut(id) {
            if d.updating_from.as_deref().is_some_and(|from| from != zeron.current) {
                d.updating_from = None;
                d.zeron_error = None;
            }
            d.zeron = Some(zeron);
            d.connected = true;
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
        if let Err(error) = engine::get().call("ApplyUpdate", params).await {
            let error = error.to_string();
            eprintln!("zeron-zgx: updates: Zeron on {device}: {error}");
            if let Some(d) = self.devices.borrow_mut().get_mut(&device) {
                d.updating_from = None;
                d.zeron_error = Some(error.clone());
            }
            self.publish();
            return Err(error);
        }
        // (Its engine restarts now: the new version comes as it's back. One
        // that never comes shows its update again.)
        let me = self.clone();
        engine::get().spawn(async move {
            zgx::timer::sleep(ZERON_UPDATE_PATIENCE).await;
            let late = me
                .devices
                .borrow_mut()
                .get_mut(&device)
                .filter(|d| d.updating_from.as_deref() == Some(from.as_str()))
                .map(|d| d.updating_from = None)
                .is_some();
            if late {
                me.publish();
            }
        });
        Ok(Value::Nil)
    }

    fn publish(&self) {
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
        for (id, d) in &order {
            rows.extend(zeron_row(id, d));
        }
        for (id, d) in &order {
            let is_here = Some(id.as_str()) == here.as_deref();
            rows.extend(d.statuses.iter().filter(|s| s.show_update_notice()).map(|s| agent_row(id, d, is_here, s)));
        }
        Value::record([("rows", Value::List(rows.into()))])
    }
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
    // (`ApplyUpdate` updates a managed install: there, its tarball's.)
    let can_apply = d.platform == "linux";
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
    if let Some(message) = message_of(status) {
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
        d.zeron = Some(Zeron { current: "1".into(), latest: Some("2".into()), available: false });
        assert!(zeron_row("m", &d).is_none());
        d.zeron.as_mut().unwrap().available = true;
        let row = zeron_row("m", &d).unwrap();
        assert_eq!(row.field("action").map(text).as_deref(), Some("update"));
        assert_eq!(row.field("latest").map(text).as_deref(), Some("2"));
        // A Mac's or a PC's app updates itself, there.
        d.platform = "macos".into();
        assert!(zeron_row("m", &d).unwrap().field("action").is_none());
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
}
