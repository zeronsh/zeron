//! Installing someone else's template, in a window of its own:
//! `ui/template_consent.luau` on a host with no plugins and no templates,
//! the only one holding `templates.decide` (the engine's `DecideTemplate`).
//! The main window's scripts, which plugins and templates may stand in for,
//! can open it (`templates.review`) but never answer it. A copy of the
//! plugins' consent window (consent.rs), kept apart so each window holds
//! only its own decision.
//!
//! It shows a package waiting in the templates folder's `.pending/` as the
//! engine lists it (`ListTemplates`' `pending`, with who signed it, looked
//! up again while Zeron is still checking).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value as Json, json};
use zgx_desktop::{WindowContext, WindowHandle, WindowOptions};
use zgx_script::{ScriptHost, Shared, Value};

use super::{Bridge, json_value};
use crate::auth::text;
use crate::engine;

/// The window, in points: the plugins' sheet's.
const SIZE: (f64, f64) = (460., 540.);
/// While who signed it is still being looked up: how often, how many times.
const RECHECK: Duration = Duration::from_millis(1000);
const RECHECKS: u32 = 10;

struct Open {
    window: WindowHandle,
    /// The package shown, by its waiting name.
    pending: Rc<RefCell<String>>,
    shown: Rc<RefCell<Option<Shared>>>,
    /// Bumped as what's shown changes: an older lookup's answer is dropped.
    generation: Rc<Cell<u64>>,
    /// The package as last shown (kept on the sheet while you answer).
    last: Last,
}

type Last = Rc<RefCell<Option<Json>>>;

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
}

fn record(item: Option<&Json>, busy: bool, error: Option<&str>) -> Value {
    let mut fields = vec![("busy", Value::from(busy))];
    if let Some(item) = item {
        fields.push(("item", json_value(item)));
    }
    if let Some(error) = error {
        fields.push(("error", Value::from(error)));
    }
    Value::record(fields)
}

/// Whether who signed it is still being looked up.
fn checking(item: &Json) -> bool {
    item["author"]["kind"] == "unknown" && item["author"]["reason"].as_str().is_some_and(|r| r.starts_with("Checking"))
}

/// Looks the package up again and shows it (and again, while who signed it
/// is still being checked).
fn fetch(shared: Shared, last: Last, pending: String, generation: Rc<Cell<u64>>, error: Option<String>) {
    let at = generation.get();
    engine::get().spawn(async move {
        for attempt in 0..=RECHECKS {
            let list = engine::get().call("ListTemplates", json!({})).await;
            if generation.get() != at {
                return;
            }
            let item = list.ok().and_then(|l| {
                l["pending"].as_array().and_then(|all| all.iter().find(|p| p["pending"] == pending.as_str()).cloned())
            });
            shared.set(record(item.as_ref(), false, error.as_deref()));
            *last.borrow_mut() = item.clone();
            if !item.as_ref().is_some_and(checking) || attempt == RECHECKS {
                return;
            }
            zgx::timer::sleep(RECHECK).await;
            if generation.get() != at {
                return;
            }
        }
    });
}

/// Shows waiting package `pending`'s sheet: in the window if it's open,
/// else in a new one.
pub fn review(bridge: &Rc<Bridge>, pending: &str) {
    if pending.is_empty() {
        return;
    }
    let reused = OPEN.with(|open| {
        let open = open.borrow();
        let Some(o) = &*open else { return false };
        *o.pending.borrow_mut() = pending.to_string();
        o.generation.set(o.generation.get() + 1);
        if let Some(shared) = &*o.shown.borrow() {
            shared.set(record(None, true, None));
            o.last.borrow_mut().take();
            fetch(shared.clone(), o.last.clone(), pending.to_string(), o.generation.clone(), None);
        }
        o.window.set_visible(true);
        o.window.request_focus();
        true
    });
    if reused {
        return;
    }
    let pending_cell = Rc::new(RefCell::new(pending.to_string()));
    let shown_cell: Rc<RefCell<Option<Shared>>> = Rc::default();
    let generation = Rc::new(Cell::new(0));
    let last: Last = Rc::default();
    let weak = Rc::downgrade(bridge);
    let (names, slot, gen_, last_) = (pending_cell.clone(), shown_cell.clone(), generation.clone(), last.clone());
    let window = bridge.windows.open(options(), move |cx| {
        let Some(bridge) = weak.upgrade() else { return };
        let host = host(cx, &bridge, names.clone(), slot.clone(), gen_.clone(), last_.clone());
        // (`ZERON_ZGX_TEMPLATE_REVIEW`, development: devtools are this window's.)
        crate::serve(cx, host, "template_consent.luau", std::env::var_os("ZERON_ZGX_TEMPLATE_REVIEW").is_some());
        cx.on_closed(move || {
            OPEN.with(|open| open.borrow_mut().take());
        });
    });
    OPEN.with(|open| {
        *open.borrow_mut() = Some(Open { window, pending: pending_cell, shown: shown_cell, generation, last });
    });
}

fn options() -> WindowOptions {
    WindowOptions {
        resizable: false,
        ..crate::zeron_window(SIZE.0, SIZE.1)
    }
}

fn host(
    cx: &mut WindowContext,
    bridge: &Rc<Bridge>,
    pending: Rc<RefCell<String>>,
    slot: Rc<RefCell<Option<Shared>>>,
    generation: Rc<Cell<u64>>,
    last: Last,
) -> ScriptHost {
    let host = ScriptHost::new(&cx.ui.runtime, crate::ui_dir().join("template_consent.luau")).app_id(crate::APP_ID);
    host.window_chrome(cx.window_info.titlebar_area.clone(), cx.window_info.controls.clone());
    host.window_zoom(&cx.ui);
    let shared = host.shared("template_consent", record(None, true, None));
    *slot.borrow_mut() = Some(shared.clone());
    fetch(shared.clone(), last.clone(), pending.borrow().clone(), generation.clone(), None);
    host.action("appearance", |value| {
        crate::set_appearance(match value {
            Value::Bool(dark) => Some(dark),
            _ => None,
        })
    });
    let window = cx.window.clone();
    host.action("close", move |_| window.close());

    // Your answer: the package as shown (its hash), installed or not. The
    // window closes once the engine has it, and the main window looks again.
    let (weak, window, out) = (Rc::downgrade(bridge), cx.window.clone(), shared);
    host.action("templates.decide", move |arg| {
        let name = arg.field("pending").map(text).unwrap_or_default();
        let allow = arg.field("allow").is_some_and(Value::truthy);
        let hash = arg.field("hash").map(text).unwrap_or_default();
        eprintln!("zeron-zgx: templates: {name} {}", if allow { "allowed" } else { "not allowed (Not now)" });
        generation.set(generation.get() + 1);
        out.set(record(last.borrow().as_ref(), true, None));
        let (weak, window, out, pending, generation, last) =
            (weak.clone(), window.clone(), out.clone(), pending.clone(), generation.clone(), last.clone());
        engine::get().spawn(async move {
            let result = engine::get()
                .call("DecideTemplate", json!({ "pending": name, "allow": allow, "hash": hash }))
                .await;
            if let Some(b) = weak.upgrade() {
                b.look();
            }
            match result {
                Ok(_) => window.close(),
                Err(error) => {
                    let name = pending.borrow().clone();
                    fetch(out, last, name, generation, Some(said(&error)));
                }
            }
        });
    });
    host
}

fn said(error: &str) -> String {
    match error {
        "offline" | "closed" => "Zeron isn't running right now. Try again in a moment.".into(),
        other => other.to_string(),
    }
}
