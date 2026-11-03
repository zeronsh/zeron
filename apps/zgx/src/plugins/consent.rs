//! Allowing a plugin, in a window of its own: `ui/consent.luau` on a host
//! with no plugins and no templates, the only one holding `plugins.decide`
//! and `plugins.sign`. The main window's scripts, which plugins and your
//! templates may stand in for, can open it (`plugins.review`) but never
//! answer it.
//!
//! A plugin on another of your devices (Settings → Plugins' switcher) is
//! allowed here too, on that device: the sheet names it, and the decision
//! goes to its engine (`targetDeviceId`).

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::json;
use zgx_desktop::{WindowContext, WindowHandle, WindowOptions};
use zgx_script::{ScriptHost, Shared, Value};

use super::{Bridge, json_value, remote};
use crate::auth::text;
use crate::engine;

/// The window, in points: room for a plugin's name and a handful of lines.
const SIZE: (f64, f64) = (460., 540.);

/// Another device: its id and its name.
type On = Option<(String, String)>;

pub struct Consent {
    window: WindowHandle,
    /// The plugin shown, by id, and the device it's on (none: this one), by
    /// id and name.
    id: Rc<RefCell<String>>,
    on: Rc<RefCell<On>>,
    shown: Rc<RefCell<Option<Shared>>>,
}

impl Consent {
    pub fn close(&self) {
        self.window.close();
    }

    /// What the window shows, as the list changes (a plugin that changed
    /// while you looked shows as it is now).
    pub fn follow(&self, bridge: &Bridge) {
        if let Some(shared) = &*self.shown.borrow() {
            shared.set(shown(bridge, &self.id.borrow(), &self.on.borrow(), false, None));
        }
    }
}

fn shown(bridge: &Bridge, id: &str, on: &On, busy: bool, error: Option<&str>) -> Value {
    let mut fields = vec![("busy", Value::from(busy))];
    let device = on.as_ref().map(|(d, _)| d.as_str());
    if let Some(item) = bridge.item_on(device, id).filter(|i| matches!(i.status(), "pending" | "tampered")) {
        fields.push(("item", json_value(&item.json)));
    }
    if let Some((_, name)) = on {
        fields.push(("device_name", Value::from(name.as_str())));
    }
    if let Some(error) = error {
        fields.push(("error", Value::from(error)));
    }
    Value::record(fields)
}

/// Shows plugin `id`'s sheet (on `on`, another device, or this one): in
/// the window if it's open, else in a new one.
pub fn review(bridge: &Rc<Bridge>, id: &str, on: On) {
    if bridge.item_on(on.as_ref().map(|(d, _)| d.as_str()), id).is_none() {
        return;
    }
    if let Some(c) = &*bridge.consent.borrow() {
        *c.id.borrow_mut() = id.to_string();
        *c.on.borrow_mut() = on;
        c.follow(bridge);
        c.window.set_visible(true);
        c.window.request_focus();
        return;
    }
    let id_cell = Rc::new(RefCell::new(id.to_string()));
    let on_cell = Rc::new(RefCell::new(on));
    let shown_cell: Rc<RefCell<Option<Shared>>> = Rc::default();
    let weak = Rc::downgrade(bridge);
    let (ids, ons, slot) = (id_cell.clone(), on_cell.clone(), shown_cell.clone());
    let window = bridge.windows.open(options(), move |cx| {
        let Some(bridge) = weak.upgrade() else { return };
        let host = host(cx, &bridge, ids.clone(), ons.clone(), slot.clone());
        // (`ZERON_ZGX_REVIEW`, development: devtools are this window's.)
        crate::serve(cx, host, "consent.luau", std::env::var_os("ZERON_ZGX_REVIEW").is_some());
        let weak = Rc::downgrade(&bridge);
        cx.on_closed(move || {
            if let Some(b) = weak.upgrade() {
                b.consent.borrow_mut().take();
            }
        });
    });
    *bridge.consent.borrow_mut() = Some(Consent { window, id: id_cell, on: on_cell, shown: shown_cell });
}

fn options() -> WindowOptions {
    WindowOptions {
        resizable: false,
        ..crate::zeron_window(SIZE.0, SIZE.1)
    }
}

fn host(cx: &mut WindowContext, bridge: &Rc<Bridge>, id: Rc<RefCell<String>>, on: Rc<RefCell<On>>, slot: Rc<RefCell<Option<Shared>>>) -> ScriptHost {
    let host = ScriptHost::new(&cx.ui.runtime, crate::ui_dir().join("consent.luau")).app_id(crate::APP_ID);
    host.window_chrome(cx.window_info.titlebar_area.clone(), cx.window_info.controls.clone());
    host.window_zoom(&cx.ui);
    let shared = host.shared("consent", shown(bridge, &id.borrow(), &on.borrow(), false, None));
    *slot.borrow_mut() = Some(shared.clone());
    host.action("appearance", |value| {
        crate::set_appearance(match value {
            Value::Bool(dark) => Some(dark),
            _ => None,
        })
    });
    let window = cx.window.clone();
    host.action("close", move |_| window.close());

    // Your answer: what was shown (its hash), allowed or not. The window
    // closes once the engine has it.
    let (weak, window, out, ons) = (Rc::downgrade(bridge), cx.window.clone(), shared.clone(), on.clone());
    host.action("plugins.decide", move |arg| {
        let Some(b) = weak.upgrade() else { return };
        let id = arg.field("id").map(text).unwrap_or_default();
        let allow = arg.field("allow").is_some_and(Value::truthy);
        let hash = arg.field("hash").map(text).unwrap_or_default();
        // (Where the sheet shown was: the decision goes there.)
        let on = ons.borrow().clone();
        eprintln!("zeron-zgx: plugins: {id} {}", if allow { "allowed" } else { "not allowed (Not now)" });
        out.set(shown(&b, &id, &on, true, None));
        let (weak, window, out) = (Rc::downgrade(&b), window.clone(), out.clone());
        engine::get().spawn(async move {
            let device = on.as_ref().map(|(d, _)| d.as_str());
            let params = remote::addressed(device, json!({ "id": id, "allow": allow, "hash": hash }));
            let result = engine::get().call("DecidePlugin", params).await;
            let Some(b) = weak.upgrade() else { return };
            match result {
                Ok(_) => window.close(),
                Err(error) => {
                    out.set(shown(&b, &id, &on, false, Some(&said(&error, &on))));
                }
            }
        });
    });
    // "It's mine": signed as yours, after which it asks only for what it
    // needs, as your own plugins do. (This device's alone: signing is with
    // its key, so another device's sheet doesn't offer it.)
    let (weak, out) = (Rc::downgrade(bridge), shared);
    host.action("plugins.sign", move |arg| {
        let Some(b) = weak.upgrade() else { return };
        if on.borrow().is_some() {
            return;
        }
        let id = arg.field("id").map(text).unwrap_or_default();
        out.set(shown(&b, &id, &None, true, None));
        let (weak, out) = (Rc::downgrade(&b), out.clone());
        engine::get().spawn(async move {
            let result = engine::get().call("SignPlugin", json!({ "id": id })).await;
            let Some(b) = weak.upgrade() else { return };
            match result {
                // The list follows with it as yours (or on, asking nothing).
                Ok(_) => {
                    out.set(shown(&b, &id, &None, false, None));
                }
                Err(error) => {
                    out.set(shown(&b, &id, &None, false, Some(&said(&error, &None))));
                }
            }
        });
    });
    host
}

fn said(error: &str, on: &On) -> String {
    match (error, on) {
        ("offline" | "closed", _) => "Zeron isn't running right now. Try again in a moment.".into(),
        (e, Some((_, name))) if remote::unreachable(e) => format!("Can't reach {name} right now. Try again once it's online."),
        (other, _) => other.to_string(),
    }
}
