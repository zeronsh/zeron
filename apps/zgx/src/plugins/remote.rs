//! Another of your devices' plugins, for Settings → Plugins' device
//! switcher. Plugins are per device (what each runs, what each allowed), and
//! the engine forwards their calls to the device they name
//! (`targetDeviceId`). What the chosen device lists is watched while it's
//! shown and published as `plugins.remote`, never as `plugins` (that is what
//! this window loads, and another device's list must never load here):
//!
//!   { device = id?, state = "idle" | "loading" | "ready" | "offline",
//!     list = { PluginItem } }   (the engine's items: their `icon_svg` its own)
//!
//! `plugins.target(id | nil)` chooses it (nil: this device, the page's own
//! list). Asked again for the same device, it looks again with the list
//! kept (a visit, the device coming back online). Every action the page takes
//! names its device itself (`device`), so a dialog confirmed after a switch
//! still acts where it was opened.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use serde_json::{Value as Json, json};
use zgx_script::{Shared, Value};

use super::{Bridge, Item, json_value, vlist};
use crate::engine::{self, rpc::Watch};

pub struct Remote {
    shared: Shared,
    target: RefCell<Option<String>>,
    state: Cell<&'static str>,
    items: RefCell<Vec<Item>>,
    watch: RefCell<Option<Watch>>,
    /// Bumped each time it's asked to look: answers to an older ask go.
    generation: Cell<u64>,
}

fn value(device: Option<&str>, state: &str, items: &[Item]) -> Value {
    let mut fields = vec![
        ("state", Value::from(state)),
        ("list", vlist(items.iter().map(|i| json_value(&i.json)).collect())),
    ];
    if let Some(device) = device {
        fields.push(("device", Value::from(device)));
    }
    Value::record(fields)
}

/// An engine error that says the device can't be reached (its link, or
/// this Mac's engine), not that the call itself failed.
pub fn unreachable(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    engine::outcome(error) == "offline" || lower.contains("cannot reach") || lower.contains("timed out") || lower.contains("timeout")
}

impl Remote {
    pub fn new(shared: Shared) -> Self {
        Self {
            shared,
            target: RefCell::default(),
            state: Cell::new("idle"),
            items: RefCell::default(),
            watch: RefCell::default(),
            generation: Cell::new(0),
        }
    }

    pub fn initial() -> Value {
        value(None, "idle", &[])
    }

    pub fn target(&self) -> Option<String> {
        self.target.borrow().clone()
    }

    /// Plugin `id` as the device shown lists it.
    pub fn item(&self, device: &str, id: &str) -> Option<Item> {
        if self.target.borrow().as_deref() != Some(device) {
            return None;
        }
        self.items.borrow().iter().find(|i| i.id() == id).cloned()
    }

    fn publish(&self) {
        self.shared.set(value(self.target.borrow().as_deref(), self.state.get(), &self.items.borrow()));
    }

    /// The device's list (`{ dirs, plugins }`), in name order as this
    /// device's is.
    fn heard(&self, frame: &Json) {
        let mut list: Vec<Item> = frame["plugins"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|json| Item { json: json.clone() })
            .collect();
        list.sort_by_key(|a| a.name().to_lowercase());
        *self.items.borrow_mut() = list;
        self.state.set("ready");
    }

    /// A call to `device` failed: when it couldn't reach it and that's the
    /// device shown, the page says it's offline.
    pub fn failed(&self, device: &str, error: &str) {
        if self.target.borrow().as_deref() == Some(device) && unreachable(error) {
            self.watch.borrow_mut().take();
            self.state.set("offline");
            self.publish();
        }
    }
}

/// Shows `target`'s plugins (none: this device's, and the watch stops). The
/// same device again looks again, its list kept meanwhile.
pub fn retarget(bridge: &Rc<Bridge>, target: Option<String>) {
    let r = &bridge.remote;
    let same = *r.target.borrow() == target;
    r.generation.set(r.generation.get() + 1);
    r.watch.borrow_mut().take();
    if !same {
        *r.target.borrow_mut() = target.clone();
        r.items.borrow_mut().clear();
        r.state.set(if target.is_some() { "loading" } else { "idle" });
        r.publish();
        if let Some(c) = &*bridge.consent.borrow() {
            c.follow(bridge);
        }
    }
    load(bridge);
}

/// Asks the device shown for its list, then watches it (again on each new
/// connection to the engine).
pub fn load(bridge: &Rc<Bridge>) {
    let Some(device) = bridge.remote.target() else { return };
    let run = bridge.remote.generation.get();
    let weak = Rc::downgrade(bridge);
    engine::get().spawn(async move {
        let reply = engine::get().call("ListPlugins", json!({ "targetDeviceId": device })).await;
        let Some(b) = weak.upgrade() else { return };
        if b.remote.generation.get() != run {
            return;
        }
        match reply {
            Ok(frame) => {
                b.remote.heard(&frame);
                b.remote.publish();
                watch(&b, &device, run);
            }
            Err(error) => {
                eprintln!("zeron-zgx: plugins: {device}'s plugins unavailable: {error}");
                b.remote.state.set("offline");
                b.remote.publish();
            }
        }
        if let Some(c) = &*b.consent.borrow() {
            c.follow(&b);
        }
    });
}

fn watch(bridge: &Rc<Bridge>, device: &str, run: u64) {
    let Some(rpc) = engine::get().current() else { return };
    let weak = Rc::downgrade(bridge);
    let handle = rpc.watch("WatchPlugins", json!({ "targetDeviceId": device }), move |frame| {
        let Some(b) = weak.upgrade() else { return };
        if b.remote.generation.get() != run {
            return;
        }
        b.remote.heard(&frame);
        b.remote.publish();
        if let Some(c) = &*b.consent.borrow() {
            c.follow(&b);
        }
    });
    *bridge.remote.watch.borrow_mut() = Some(handle);
}

/// The most of a plugin file one call carries to another device (decoded):
/// a request crosses the relay as one message, so a file goes in pieces
/// (the engine's `plugins::uploads::PIECE`).
pub const PIECE: usize = 384 << 10;
/// The largest plugin file sent (the engine's package limit, rounded).
const MAX_FILE: u64 = 64 << 20;

/// Sends the `.zeron-plugin` file at `path` (on this device) to another,
/// piece by piece through `stage` (`StagePluginPackage` there): the upload
/// it's held as there, for `install_sent`.
pub async fn send_file<F, Fut>(path: &str, stage: F) -> Result<String, String>
where
    F: Fn(Json) -> Fut,
    Fut: std::future::Future<Output = Result<Json, String>>,
{
    use base64::Engine as _;
    let meta = std::fs::metadata(path).map_err(|_| "That file can't be found.".to_string())?;
    if !meta.is_file() {
        return Err("That isn't a file.".into());
    }
    if meta.len() > MAX_FILE {
        return Err("That plugin file is too big".into());
    }
    let raw = std::fs::read(path).map_err(|e| format!("Couldn't read it: {e}"))?;
    let mut upload = Json::Null;
    // (An empty file is still sent, for the engine to say what it isn't.)
    let pieces: Vec<&[u8]> = if raw.is_empty() { vec![&[][..]] } else { raw.chunks(PIECE).collect() };
    for piece in pieces {
        let staged = stage(json!({ "upload": upload, "data": base64::engine::general_purpose::STANDARD.encode(piece) })).await?;
        upload = staged["upload"].clone();
    }
    upload.as_str().map(str::to_owned).ok_or_else(|| "The other device didn't take the file".into())
}

/// `InstallPlugin`'s params for a file sent there: the upload, and the
/// file's name (never its folder here).
pub fn install_sent(path: &str, upload: &str) -> Json {
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    json!({ "upload": upload, "name": name })
}

/// `params` addressed to `device` (none: this one's engine).
pub fn addressed(device: Option<&str>, mut params: Json) -> Json {
    if let (Some(device), Some(object)) = (device, params.as_object_mut()) {
        object.insert("targetDeviceId".into(), json!(device));
    }
    params
}

/// The device an action names (`{ device = id }`), if it names one.
pub fn device_of(arg: &Value) -> Option<String> {
    arg.field("device").map(crate::auth::text).filter(|d| !d.is_empty())
}

/// An action's plugin and device: `id`, or `{ id, device? }`.
pub fn id_and_device(arg: &Value) -> (String, Option<String>) {
    match arg.field("id") {
        Some(id) => (crate::auth::text(id), device_of(arg)),
        None => (crate::auth::text(arg), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_are_addressed_only_to_another_device() {
        assert_eq!(addressed(Some("devbox"), json!({ "id": "focus" })), json!({ "id": "focus", "targetDeviceId": "devbox" }));
        assert_eq!(addressed(None, json!({ "id": "focus" })), json!({ "id": "focus" }));
    }

    #[test]
    fn an_action_names_its_plugin_and_device() {
        let arg = Value::record([("id", Value::from("focus")), ("device", Value::from("devbox"))]);
        assert_eq!(id_and_device(&arg), ("focus".into(), Some("devbox".into())));
        assert_eq!(id_and_device(&Value::from("focus")), ("focus".into(), None));
        let here = Value::record([("id", Value::from("focus")), ("device", Value::from(""))]);
        assert_eq!(id_and_device(&here), ("focus".into(), None));
    }

    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(future)
    }

    /// A file here installed on another device goes there in pieces, and
    /// installs as what it was sent as, by its name alone.
    #[test]
    fn a_file_goes_to_another_device_in_pieces() {
        use base64::Engine as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("focus.zeron-plugin");
        let raw: Vec<u8> = (0..(PIECE * 2 + 10)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &raw).unwrap();
        let sent = RefCell::new(Vec::<Json>::new());
        let upload = block_on(send_file(path.to_str().unwrap(), |piece| {
            sent.borrow_mut().push(piece);
            async { Ok(json!({ "upload": "u1" })) }
        }))
        .unwrap();
        assert_eq!(upload, "u1");
        let sent = sent.into_inner();
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[0]["upload"], Json::Null, "a new upload");
        assert!(sent[1..].iter().all(|p| p["upload"] == "u1"), "then added to");
        let back: Vec<u8> = sent
            .iter()
            .flat_map(|p| base64::engine::general_purpose::STANDARD.decode(p["data"].as_str().unwrap()).unwrap())
            .collect();
        assert_eq!(back, raw);
        assert_eq!(
            install_sent(path.to_str().unwrap(), "u1"),
            json!({ "upload": "u1", "name": "focus.zeron-plugin" })
        );
        // The device saying no stops it there.
        let refused = block_on(send_file(path.to_str().unwrap(), |_| async { Err("offline".to_string()) }));
        assert_eq!(refused, Err("offline".into()));
    }

    #[test]
    fn a_device_out_of_reach_reads_as_offline() {
        assert!(unreachable("cannot reach device d1: remote routing unavailable (offline)"));
        assert!(unreachable("offline"));
        // (As the engine says it, the two-engine rig's device-z.)
        assert!(unreachable("transport: peer device-z: unreachable (backing off after 1 failed dials; retrying in ~4s)"));
        assert!(!unreachable("Another device installs a plugin from a link (url), not a file on this one"));
    }
}
