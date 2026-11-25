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
use std::time::{Duration, Instant};

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
    static SEEN: RefCell<Option<(String, Instant)>> = const { RefCell::new(None) };
}

/// The sheet shown, and since when: an Allow is taken only of what has shown
/// a moment (`consent::ARMED`).
fn saw(item: Option<&Json>) {
    let key = item.map(sheet);
    SEEN.with(|s| {
        let mut s = s.borrow_mut();
        if s.as_ref().map(|(shown, _)| shown) != key.as_ref() {
            *s = key.map(|key| (key, Instant::now()));
        }
    });
}

/// Which sheet `item` is: what's asked of, and as what.
fn sheet(item: &Json) -> String {
    let field = |k: &str| item[k].as_str().unwrap_or_default();
    // (And what the store's word on it is drawn as: a new word is a sheet to
    // be read again.)
    let said = drawn(item).map(|d| d.to_string()).unwrap_or_default();
    format!("{}{said}", key(field("keep"), field("pending"), field("hash")))
}

fn key(keep: &str, pending: &str, hash: &str) -> String {
    format!("{keep}/{pending}@{hash}")
}

/// The glyphs that read as the store's mark (the engine's `mark::MARK_GLYPHS`).
const MARK_GLYPHS: &[char] = &['\u{2713}', '\u{2714}', '\u{2611}', '\u{2705}', '\u{1F5F8}', '\u{1F5F9}', '\u{221A}', '\u{10102}', '\u{1FBB1}'];

/// `item` as the sheet shows it: its own words one line of what prints, and
/// short (words that read as Zeron's own, "✓ Verified", among them); a version
/// only if it reads as one.
fn shown(item: &Json) -> Json {
    let mut out = item.clone();
    for (key, most) in [("name", 60), ("description", 300), ("id", 64), ("zeron_version", 32)] {
        if let Some(text) = item[key].as_str() {
            out[key] = json!(super::consent::plain(text, most));
        }
    }
    // (The store's refuses one in a name; a file's or a link's is its own.)
    if !item["store"].is_object()
        && let Some(name) = out["name"].as_str()
    {
        let name: String = name.chars().filter(|c| !MARK_GLYPHS.contains(c)).collect();
        out["name"] = json!(super::consent::plain(&name, 60));
    }
    if out["name"].as_str().is_none_or(str::is_empty) {
        out["name"] = out["id"].clone();
    }
    // (A version begins with a number: a word in its place is no version.)
    // (The store's limit, 32, for one of the store's; a file's or a link's may
    // carry more, drawn cut short.)
    let most = if item["store"].is_object() { 32 } else { 128 };
    let reads = |v: &str| (1..=most).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'));
    if item["version"].as_str().is_some_and(|v| !reads(v) || !v.starts_with(|c: char| c.is_ascii_digit())) {
        out["version"] = json!("");
    }
    if let Some(files) = item["ignored"].as_array() {
        out["ignored_n"] = json!(files.len());
        out["ignored"] = files.iter().filter_map(Json::as_str).take(20).map(|f| super::consent::plain(f, 120)).collect::<Vec<_>>().into();
    }
    out
}

/// What the window draws of the store's word on `item`, for the engine to hold
/// the store's record to (`ReviewTemplate`'s `shown`): from what's drawn, never
/// from what a script says of it. None for one that isn't the store's.
fn drawn(item: &Json) -> Option<Json> {
    let item = shown(item);
    item["store"].is_object().then(|| {
        json!({
            "verified": item["store"]["verified"] == true,
            "publisher": item["store"]["publisher"],
            "publisher_id": item["store"]["publisher_id"],
            "version": item["version"],
        })
    })
}

/// `item` as the store says of it now (`changed`, `ReviewTemplate`'s), and
/// which of it changed since it was shown.
fn restate(item: &mut Json, changed: &Json) {
    let was = item["store"].clone();
    for key in ["verified", "verified_at", "publisher", "publisher_id"] {
        item["store"][key] = changed[key].clone();
    }
    if let Some(username) = changed["publisher"].as_str() {
        item["author"] = json!({ "kind": "verified", "username": username });
    }
    let publisher = ["publisher", "publisher_id"].iter().any(|key| !was[*key].is_null() && was[*key] != changed[*key]);
    item["store_changed"] = json!({
        "verified": was["verified"] != true && changed["verified"] == true,
        "unverified": was["verified"] == true && changed["verified"] != true,
        "publisher": publisher,
    });
}

fn record(item: Option<&Json>, busy: bool, error: Option<&str>) -> Value {
    saw(item);
    let item = item.map(shown);
    let item = item.as_ref();
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
    let files = super::live::files(&dir, zeron_plugin_guard::template::counted).len();
    let author = match &t.imported {
        // (The store's, kept: what's there now isn't known to be its
        // publisher's, and they're never named over what they didn't make.)
        Some(imported) if imported["source"]["kind"] == "store" => {
            json!({ "kind": "unknown", "reason": "From the store. Its files may have been changed on this device since." })
        }
        // (A file's or a link's the same: it's kept because it changed, or
        // was never kept, so its signer isn't named over it.)
        Some(_) => json!({ "kind": "unknown", "reason": "Installed from a file or a link. Its files may have been changed on this device since." }),
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
        crate::serve_sheet(cx, host, "template_consent.luau", crate::dev::switch("ZERON_ZGX_TEMPLATE_REVIEW").is_some());
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
    crate::transparency::install(cx, &host);
    let shared = host.shared("template_consent", record(None, true, None));
    *slot.borrow_mut() = Some(shared.clone());
    fetch(shared.clone(), last.clone(), pending.borrow().clone(), generation.clone(), None);
    let window = cx.window.clone();
    host.action("appearance", move |value| {
        crate::set_appearance(
            &window,
            match value {
                Value::Bool(dark) => Some(dark),
                _ => None,
            },
        )
    });
    let window = cx.window.clone();
    host.action("close", move |_| window.close());

    install_verified(&host, bridge, &pending, &shared, &generation, &last);
    // Your answer: the package as shown (its hash), installed or not. The
    // window closes once the engine has it, and the main window looks again.
    let (weak, window, out) = (Rc::downgrade(bridge), cx.window.clone(), shared);
    host.action("templates.decide", move |arg| {
        let allow = arg.field("allow").is_some_and(Value::truthy);
        let hash = arg.field("hash").map(text).unwrap_or_default();
        // (Before anything: a click or an Enter meant for the main window, as
        // this one comes up under it, allows nothing.)
        let of = |k: &str| arg.field(k).map(text).unwrap_or_default();
        let asked = key(&of("keep"), &of("pending"), &hash);
        if allow && !SEEN.with(|s| super::consent::may_allow(&s.borrow(), &asked, Instant::now())) {
            return;
        }
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
                        if super::blocks::blocked(&root, &id).is_some() {
                            return Err("Zeron took this template down, or can't check it now.".into());
                        }
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
        let asked_at = generation.get();
        let drawn = last.borrow().as_ref().and_then(drawn);
        out.set(record(last.borrow().as_ref(), true, None));
        let (weak, window, out, pending, generation, last) =
            (weak.clone(), window.clone(), out.clone(), pending.clone(), generation.clone(), last.clone());
        engine::get().spawn(async move {
            // Allowing it: the engine's single-use review for this package,
            // minted here (never by a script) just before the answer
            // (`ReviewTemplate`, the engine authority's).
            let mut params = json!({ "pending": name, "allow": allow, "hash": hash });
            if allow {
                let review = crate::auth::after_setup(|| {
                    let mut asked = json!({ "pending": name, "hash": hash });
                    if let Some(drawn) = &drawn {
                        asked["shown"] = drawn.clone();
                    }
                    async move { engine::get().call("ReviewTemplate", asked).await }
                })
                .await;
                // (Another package came to show meanwhile: this Allow is
                // nobody's, and what came of it isn't that one's.)
                if generation.get() != asked_at || *pending.borrow() != name {
                    return;
                }
                match review {
                    // What the store says of it changed since it was shown:
                    // shown as it is now, and asked again (a second Allow).
                    Ok(reply) if reply["changed"].is_object() => {
                        ask_again(&out, &last, &reply["changed"]);
                        return;
                    }
                    Ok(reply) => params["review"] = reply["review"].clone(),
                    // The store's: taken down, gone, or not to be asked. Said
                    // in the sheet, which stays; nothing is allowed.
                    Err(error) if last.borrow().as_ref().is_some_and(|item| item["store"].is_object()) => {
                        eprintln!("zeron-zgx: templates: {name} isn't allowed: {error}");
                        let item = last.borrow().clone();
                        out.set(record(item.as_ref(), false, Some(&said(&error))));
                        return;
                    }
                    Err(error) => eprintln!("zeron-zgx: templates: no review for {name}: {error}"),
                }
            }
            let result = crate::auth::after_setup(|| {
                let asked = params.clone();
                async move { engine::get().call("DecideTemplate", asked).await }
            })
            .await;
            // (The store's word changed between the two: nothing's installed,
            // and it's asked again, unless another shows by now.)
            if let Ok(reply) = &result
                && reply["changed"].is_object()
            {
                if generation.get() == asked_at && *pending.borrow() == name {
                    ask_again(&out, &last, &reply["changed"]);
                }
                return;
            }
            // Allowed in this window: installed as you were shown it, so it
            // runs with Zeron's powers (its files as installed).
            if let Ok(reply) = &result
                && let Some(id) = reply["installed"].as_str()
                && let Some(root) = super::templates_root()
                // (The package's, as the engine says it: never read again
                // from the disk. An engine that doesn't say: as before.)
                // (Taken down by now: nothing of it is allowed.)
                && super::blocks::blocked(&root, id).is_none()
                && let digest = reply["digest"].as_str().map_or_else(|| super::preview::digest(&root.join(id)), str::to_owned)
                && let Err(error) = super::preview::approve(id, &digest)
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

/// The sheet again, as the store says of it now: its wait started anew, for a
/// second Allow.
fn ask_again(out: &Shared, last: &Last, changed: &Json) {
    let mut item = last.borrow().clone();
    if let Some(item) = item.as_mut().filter(|item| item["store"].is_object()) {
        restate(item, changed);
    }
    SEEN.with(|s| s.borrow_mut().take());
    out.set(record(item.as_ref(), false, None));
    *last.borrow_mut() = item;
}

/// The verified version of waiting package `pending`, taken instead of it (the
/// one the engine's own item names): it waits in its place, its sheet shown.
fn install_verified(host: &ScriptHost, bridge: &Rc<Bridge>, pending: &Rc<RefCell<String>>, out: &Shared, generation: &Rc<Cell<u64>>, last: &Last) {
    let (weak, out, pending, generation, last) = (Rc::downgrade(bridge), out.clone(), pending.clone(), generation.clone(), last.clone());
    host.action("templates.install_verified", move |_| {
        // (What's shown, as it's shown: never what a script names.)
        let Some(item) = last.borrow().clone().filter(|item| item["store"]["verified_alternative"].is_object()) else { return };
        let (Some(name), Some(hash)) = (item["pending"].as_str(), item["hash"].as_str()) else { return };
        if !SEEN.with(|s| super::consent::may_allow(&s.borrow(), &key("", name, hash), Instant::now())) {
            return;
        }
        let params = json!({ "pending": name, "hash": hash });
        generation.set(generation.get() + 1);
        out.set(record(Some(&item), true, None));
        let (weak, out, pending, generation, last) = (weak.clone(), out.clone(), pending.clone(), generation.clone(), last.clone());
        engine::get().spawn(async move {
            let result = crate::auth::after_setup(|| {
                let asked = params.clone();
                async move { engine::get().call("InstallVerifiedTemplate", asked).await }
            })
            .await;
            if let Some(b) = weak.upgrade() {
                b.look();
            }
            let error = match result {
                Ok(reply) => {
                    if let Some(next) = reply["pending"].as_str() {
                        *pending.borrow_mut() = next.to_string();
                    }
                    None
                }
                Err(error) => Some(said(&error)),
            };
            let name = pending.borrow().clone();
            fetch(out, last, name, generation, error);
        });
    });
}

fn said(error: &str) -> String {
    match error {
        "offline" | "closed" => "Zeron isn't running right now. Try again in a moment.".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the engine holds the store's record to is what's drawn, and only
    /// for the store's.
    #[test]
    fn the_engine_is_told_what_the_window_draws() {
        let item = json!({ "pending": "midnight-1", "version": "1.2.0", "store": { "id": "midnight", "verified": true, "publisher": "ada", "publisher_id": "u_1", "version": "9.9.9" }, "author": { "kind": "verified", "username": "ada" } });
        assert_eq!(drawn(&item), Some(json!({ "verified": true, "publisher": "ada", "publisher_id": "u_1", "version": "1.2.0" })));
        let mut now = item.clone();
        restate(&mut now, &json!({ "verified": false, "publisher": "grace", "publisher_id": "u_2" }));
        assert_eq!(drawn(&now), Some(json!({ "verified": false, "publisher": "grace", "publisher_id": "u_2", "version": "1.2.0" })));
        // Drawn unverified, whatever isn't plainly true; no version drawn, none said.
        let mut odd = item.clone();
        odd["store"]["verified"] = json!("yes");
        odd["version"] = json!("Verified");
        assert_eq!(drawn(&odd).unwrap()["verified"], false);
        assert_eq!(drawn(&odd).unwrap()["version"], "");
        assert_eq!(drawn(&json!({ "pending": "x", "version": "1.0.0" })), None);
    }

    /// What the store came to say of it is shown, and which of it changed.
    #[test]
    fn what_the_store_came_to_say_is_shown_and_said() {
        let item = json!({ "pending": "midnight-1", "hash": "h", "store": { "id": "midnight", "verified": true, "verified_at": 5, "publisher": "ada", "publisher_id": "u_1" }, "author": { "kind": "verified", "username": "ada" } });
        let mut now = item.clone();
        restate(&mut now, &json!({ "verified": false, "publisher": "grace", "publisher_id": "u_2" }));
        assert_eq!(now["store"], json!({ "id": "midnight", "verified": false, "verified_at": null, "publisher": "grace", "publisher_id": "u_2" }));
        assert_eq!(now["author"]["username"], "grace");
        assert_eq!(now["store_changed"], json!({ "verified": false, "unverified": true, "publisher": true }));
        // The same name over another account is another publisher.
        let mut now = item.clone();
        restate(&mut now, &json!({ "verified": true, "publisher": "ada", "publisher_id": "u_9" }));
        assert_eq!(now["store_changed"], json!({ "verified": false, "unverified": false, "publisher": true }));
        let mut now = item;
        now["store"]["verified"] = json!(false);
        restate(&mut now, &json!({ "verified": true, "verified_at": 9, "publisher": "ada", "publisher_id": "u_1" }));
        assert_eq!(now["store_changed"], json!({ "verified": true, "unverified": false, "publisher": false }));
    }

    /// What a package says of itself can't read as Zeron's own words.
    #[test]
    fn a_package_s_words_are_bounded_and_plain() {
        let item = shown(&json!({
            "name": "Midnight\n\u{202e}✓ Verified", "version": "Verified", "description": format!("a\tb{}", "x".repeat(400)),
            "ignored": ["a\nb", 5], "key": "k", "hash": "h", "id": "midnight", "zeron_version": "0.3.18",
        }));
        assert_eq!(item["name"], "Midnight Verified");
        assert_eq!(item["version"], "");
        let long = format!("1.0.0-{}", "a".repeat(122));
        assert_eq!(shown(&json!({ "version": long }))["version"], long.as_str(), "a file's, up to 128");
        assert_eq!(shown(&json!({ "version": format!("{long}b") }))["version"], "");
        let store = |v: &str| shown(&json!({ "version": v, "store": { "id": "midnight" } }))["version"].clone();
        assert_eq!((store(&"1".repeat(32)), store(&"1".repeat(33))), (json!("1".repeat(32)), json!("")), "the store's 32");
        assert_eq!(item["description"].as_str().unwrap().chars().count(), 300);
        assert_eq!(item["ignored_n"], 2, "all of them counted");
        assert_eq!(item["ignored"], json!(["a b"]));
        assert_eq!((&item["key"], &item["hash"]), (&json!("k"), &json!("h")), "what's answered with is as it was");
        assert_eq!(shown(&json!({ "name": "\u{200b}", "id": "midnight", "version": "1.2.0-beta.1" }))["name"], "midnight");
        assert_eq!(shown(&json!({ "version": "1.2.0-beta.1" }))["version"], "1.2.0-beta.1");
    }
}
