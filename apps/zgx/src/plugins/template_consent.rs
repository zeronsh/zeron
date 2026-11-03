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
    // Keeping a template in preview: what it is now, read here.
    if let Some(id) = pending.strip_prefix(KEEP) {
        let item = keep_item(id);
        shared.set(record(item.as_ref(), false, error.as_deref()));
        *last.borrow_mut() = item;
        return;
    }
    engine::get().spawn(async move {
        for attempt in 0..=RECHECKS {
            let list = crate::auth::after_setup(|| async { engine::get().call("ListTemplates", json!({})).await }).await;
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

/// What's shown names a template to keep, not a package waiting: `keep:<id>`
/// (a package's waiting name is never so: `valid_pending` has no `:`).
const KEEP: &str = "keep:";

/// Template `id` as the keep sheet shows it: its name and description, its
/// own files' count, the digest you keep (`preview::digest`), and who signed
/// it if it came from a package. None when it isn't installed (or is
/// Zeron's own, which needs no keeping).
fn keep_item(id: &str) -> Option<Json> {
    let root = super::templates_root()?;
    if id == super::live::BUILTIN || !super::live::plain(id) {
        return None;
    }
    let t = super::live::installed(&root).into_iter().find(|t| t.id == id)?;
    let dir = root.join(id);
    let files = super::live::files(&dir, |rel| !super::live::template_meta(rel) && !super::live::never_templated(rel)).len();
    let author = match &t.imported {
        Some(imported) => imported["author"].clone(),
        None => json!({ "kind": "unknown", "reason": "Made on this device, by you or an agent." }),
    };
    Some(json!({
        "keep": id,
        "id": id,
        "name": t.name,
        "description": t.description,
        "version": t.version,
        "files": files,
        "hash": super::preview::digest(&dir),
        "author": author,
    }))
}

/// The keep sheet for template `id` (one in preview): in the window if it's
/// open, else in a new one. Only its Keep (`templates.decide { keep }`)
/// gives the template Zeron's powers.
pub fn keep(bridge: &Rc<Bridge>, id: &str) {
    if id.is_empty() {
        return;
    }
    review(bridge, &format!("{KEEP}{id}"));
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
        crate::serve(cx, host, "template_consent.luau", crate::dev::switch("ZERON_ZGX_TEMPLATE_REVIEW").is_some());
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
        let allow = arg.field("allow").is_some_and(Value::truthy);
        let hash = arg.field("hash").map(text).unwrap_or_default();
        // Keeping a template in preview: recorded here, by the app alone
        // (never the engine), bound to its files as you were shown them.
        if let Some(id) = arg.field("keep").map(text).filter(|id| !id.is_empty()) {
            let shown = pending.borrow().strip_prefix(KEEP) == Some(id.as_str());
            let kept = if !allow {
                Ok(())
            } else if !shown {
                Err("This isn't the template you were shown. Open its Keep again.".to_string())
            } else {
                super::templates_root()
                    .ok_or_else(|| "Templates aren't available right now".to_string())
                    .and_then(|root| {
                        if super::preview::digest(&root.join(&id)) != hash {
                            return Err("This template changed while you looked at it. Look at it again.".into());
                        }
                        super::preview::approve(&id, &hash)
                    })
            };
            eprintln!("zeron-zgx: templates: {id} {}", if allow { "kept" } else { "not kept (Not now)" });
            match kept {
                Ok(()) => {
                    if let Some(b) = weak.upgrade() {
                        b.look();
                    }
                    window.close();
                }
                Err(error) => {
                    generation.set(generation.get() + 1);
                    let item = keep_item(&id);
                    out.set(record(item.as_ref(), false, Some(&error)));
                    *last.borrow_mut() = item;
                }
            }
            return;
        }
        let name = arg.field("pending").map(text).unwrap_or_default();
        eprintln!("zeron-zgx: templates: {name} {}", if allow { "allowed" } else { "not allowed (Not now)" });
        generation.set(generation.get() + 1);
        out.set(record(last.borrow().as_ref(), true, None));
        let (weak, window, out, pending, generation, last) =
            (weak.clone(), window.clone(), out.clone(), pending.clone(), generation.clone(), last.clone());
        engine::get().spawn(async move {
            // Allowing it: the engine's single-use review for this package,
            // minted here (never by a script) just before the answer
            // (`ReviewTemplate`, the engine authority's).
            let mut params = json!({ "pending": name, "allow": allow, "hash": hash });
            if allow {
                match crate::auth::after_setup(|| {
                    let asked = json!({ "pending": name, "hash": hash });
                    async move { engine::get().call("ReviewTemplate", asked).await }
                })
                .await {
                    Ok(reply) => params["review"] = reply["review"].clone(),
                    Err(error) => eprintln!("zeron-zgx: templates: no review for {name}: {error}"),
                }
            }
            let result = crate::auth::after_setup(|| {
                let asked = params.clone();
                async move { engine::get().call("DecideTemplate", asked).await }
            })
            .await;
            // Allowed in this window: installed as you were shown it, so it
            // runs with Zeron's powers (its files as installed).
            if let Ok(reply) = &result
                && let Some(id) = reply["installed"].as_str()
                && let Some(root) = super::templates_root()
                && let Err(error) = super::preview::approve(id, &super::preview::digest(&root.join(id)))
            {
                eprintln!("zeron-zgx: templates: {id} installed, not kept: {error}");
            }
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
