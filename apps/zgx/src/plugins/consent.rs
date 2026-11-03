//! Allowing a plugin, in a window of its own: `ui/consent.luau` on a host
//! with no plugins and no templates, the only one holding `plugins.decide`
//! and `plugins.sign`. The main window's scripts, which plugins and your
//! templates may stand in for, can open it (`plugins.review`) but never
//! answer it.
//!
//! A plugin on another of your devices (Settings → Plugins' switcher) is
//! shown here, named for that device, and can be turned down from here
//! (the decision goes to its engine, `targetDeviceId`); it's allowed on
//! that device alone, in its own window there (its engine refuses an
//! Allow that came over the relay).
//!
//! What a sheet shows is what's decided: its Allow carries a review the
//! host mints from the engine for the plugin at the hash shown
//! (`ReviewPlugin`), taken only once the sheet has shown a moment
//! (`ARMED`), and a plugin's own words reach it bounded and plain
//! (`bounded`).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};
use zgx_desktop::{WindowContext, WindowHandle, WindowOptions};
use zgx_script::{ScriptHost, Shared, Value};

use super::{Bridge, json_value, remote};
use crate::auth::text;
use crate::engine;

/// The window, in points: room for a plugin's name and a handful of lines.
const SIZE: (f64, f64) = (460., 540.);

/// Another device: its id and its name.
type On = Option<(String, String)>;

/// How long a sheet shows before the host takes its Allow: a window that
/// opened under a click mid-way never takes it. (The sheet arms its button
/// a little later, screens/plugins/consent.luau `ARM_MS`.)
const ARMED: Duration = Duration::from_millis(600);

/// What the sheet shows of a plugin's own words, at most: a name, a
/// version, a description, a tool's words.
const NAME: usize = 60;
const VERSION: usize = 32;
const WORDS: usize = 400;

/// The sheet shown now: which plugin, at which hash, since when.
type Seen = Rc<RefCell<Option<(String, Instant)>>>;

pub struct Consent {
    window: WindowHandle,
    /// The plugin shown, by id, and the device it's on (none: this one), by
    /// id and name.
    id: Rc<RefCell<String>>,
    on: Rc<RefCell<On>>,
    shown: Rc<RefCell<Option<Shared>>>,
    seen: Seen,
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
            see(&self.seen, bridge, &self.id.borrow(), &self.on.borrow());
        }
    }
}

/// The plugin a sheet may show: one waiting for you (or changed since).
fn showable(bridge: &Bridge, id: &str, on: &On) -> Option<super::Item> {
    let device = on.as_ref().map(|(d, _)| d.as_str());
    bridge.item_on(device, id).filter(|i| matches!(i.status(), "pending" | "tampered"))
}

/// Notes when the sheet started showing what it shows now (a new plugin,
/// or this one changed).
fn see(seen: &Seen, bridge: &Bridge, id: &str, on: &On) {
    let key = showable(bridge, id, on).map(|item| key(on, &item));
    let mut seen = seen.borrow_mut();
    match (key, &*seen) {
        (Some(key), Some((was, _))) if *was == key => {}
        (Some(key), _) => *seen = Some((key, Instant::now())),
        (None, _) => *seen = None,
    }
}

/// Which sheet: the device, the plugin and what it is now.
fn key(on: &On, item: &super::Item) -> String {
    format!("{}/{}@{}", on.as_ref().map_or("", |(d, _)| d.as_str()), item.id(), item.hash())
}

/// `text` shown as a plugin's words: one line of what prints, no
/// direction overrides or invisible marks (words that read as Zeron's own,
/// "✓ Zeron", among them), at most `most` characters.
fn plain(text: &str, most: usize) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control() && !invisible(*c))
        .collect::<String>()
        .split(' ')
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if out.chars().count() > most {
        out = out.chars().take(most - 1).collect::<String>() + "…";
    }
    out
}

/// Marks that change how text around them reads but show nothing of
/// their own: direction overrides and isolates, joiners, the BOM.
fn invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' | '\u{061C}')
}

/// The plugin as the sheet shows it: its own words bounded and plain
/// (`plain`), its version only if it reads as one.
fn bounded(item: &Json) -> Json {
    let mut item = item.clone();
    let words = |v: &mut Json, most: usize| {
        if let Some(text) = v.as_str() {
            *v = json!(plain(text, most));
        }
    };
    words(&mut item["name"], NAME);
    words(&mut item["description"], WORDS);
    let version = item["version"].as_str().unwrap_or("");
    let reads = (1..=VERSION).contains(&version.len())
        && version.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'));
    if !reads {
        item["version"] = json!("");
    }
    let tools = |list: &mut Json| {
        for tool in list.as_array_mut().into_iter().flatten() {
            words(&mut tool["name"], NAME);
            words(&mut tool["description"], WORDS);
        }
    };
    tools(&mut item["tools"]);
    // How far each program and place it names reaches, as the engine
    // rates them (zeron-plugin-guard): a program that can do anything you
    // can, a host on this device or your network.
    let anything = |list: &Json| -> Json {
        json!(list.as_array().into_iter().flatten().filter_map(Json::as_str).filter(|c| zeron_plugin_guard::commands::risk(c) == zeron_plugin_guard::commands::Risk::Anything).collect::<Vec<_>>())
    };
    let local = |list: &Json| -> Json {
        json!(list.as_array().into_iter().flatten().filter_map(Json::as_str).filter(|h| *h != "*" && zeron_plugin_guard::net::risky(h)).collect::<Vec<_>>())
    };
    item["commands_anything"] = anything(&item["commands"]);
    item["network_local"] = local(&item["network_hosts"]);
    if let Some(diff) = item.get_mut("digest_diff") {
        diff["commands_anything"] = anything(&diff["added_commands"]);
        diff["network_local"] = local(&diff["network_added"]);
        tools(&mut diff["tools_added"]);
        for tool in diff["tools_changed"].as_array_mut().into_iter().flatten() {
            words(&mut tool["name"], NAME);
            words(&mut tool["description_before"], WORDS);
            words(&mut tool["description_after"], WORDS);
        }
    }
    item
}

fn shown(bridge: &Bridge, id: &str, on: &On, busy: bool, error: Option<&str>) -> Value {
    let mut fields = vec![("busy", Value::from(busy))];
    if let Some(item) = showable(bridge, id, on) {
        fields.push(("item", json_value(&bounded(&item.json))));
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
    let seen: Seen = Rc::default();
    let weak = Rc::downgrade(bridge);
    let (ids, ons, slot, seen_) = (id_cell.clone(), on_cell.clone(), shown_cell.clone(), seen.clone());
    let window = bridge.windows.open(options(), move |cx| {
        let Some(bridge) = weak.upgrade() else { return };
        let host = host(cx, &bridge, ids.clone(), ons.clone(), slot.clone(), seen_.clone());
        // (`ZERON_ZGX_REVIEW`, development: devtools are this window's.)
        crate::serve(cx, host, "consent.luau", crate::dev::switch("ZERON_ZGX_REVIEW").is_some());
        let weak = Rc::downgrade(&bridge);
        cx.on_closed(move || {
            if let Some(b) = weak.upgrade() {
                b.consent.borrow_mut().take();
            }
        });
    });
    *bridge.consent.borrow_mut() = Some(Consent { window, id: id_cell, on: on_cell, shown: shown_cell, seen });
}

fn options() -> WindowOptions {
    WindowOptions {
        resizable: false,
        ..crate::zeron_window(SIZE.0, SIZE.1)
    }
}

fn host(cx: &mut WindowContext, bridge: &Rc<Bridge>, id: Rc<RefCell<String>>, on: Rc<RefCell<On>>, slot: Rc<RefCell<Option<Shared>>>, seen: Seen) -> ScriptHost {
    let host = ScriptHost::new(&cx.ui.runtime, crate::ui_dir().join("consent.luau")).app_id(crate::APP_ID);
    host.window_chrome(cx.window_info.titlebar_area.clone(), cx.window_info.controls.clone());
    host.window_zoom(&cx.ui);
    let shared = host.shared("consent", shown(bridge, &id.borrow(), &on.borrow(), false, None));
    see(&seen, bridge, &id.borrow(), &on.borrow());
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
    // closes once the engine has it. Only of the sheet shown now, as it
    // is (`seen`), and an Allow only once it has shown a moment (`ARMED`)
    // and only on this device: another device's is allowed there alone.
    let (weak, window, out, ons, ids, seen_) = (Rc::downgrade(bridge), cx.window.clone(), shared.clone(), on.clone(), id.clone(), seen.clone());
    host.action("plugins.decide", move |arg| {
        let Some(b) = weak.upgrade() else { return };
        let id = arg.field("id").map(text).unwrap_or_default();
        let allow = arg.field("allow").is_some_and(Value::truthy);
        let hash = arg.field("hash").map(text).unwrap_or_default();
        // (Where the sheet shown was: the decision goes there.)
        let on = ons.borrow().clone();
        let Some(item) = showable(&b, &id, &on).filter(|item| *ids.borrow() == id && item.hash() == hash) else {
            return;
        };
        if allow && !may_allow(&seen_.borrow(), &key(&on, &item), on.is_some(), Instant::now()) {
            return;
        }
        eprintln!("zeron-zgx: plugins: {id} {}", if allow { "allowed" } else { "not allowed (Not now)" });
        out.set(shown(&b, &id, &on, true, None));
        let (weak, window, out) = (Rc::downgrade(&b), window.clone(), out.clone());
        engine::get().spawn(async move {
            let device = on.as_ref().map(|(d, _)| d.as_str());
            // An Allow carries a review of what was shown, minted here, now
            // (single-use, for this plugin at this hash): only this
            // window's host holds one, never a script.
            let mut params = json!({ "id": id, "allow": allow, "hash": hash });
            if allow {
                match mint_review(&id, &hash).await {
                    Ok(review) => params["review"] = json!(review),
                    Err(error) => {
                        if let Some(b) = weak.upgrade() {
                            out.set(shown(&b, &id, &on, false, Some(&said(&error, &on))));
                        }
                        return;
                    }
                }
            }
            let params = remote::addressed(device, params);
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
    // (Only the plugin shown, by someone Zeron can't name, unchanged since
    // you allowed it: never one someone else signed.)
    let (weak, out, ids) = (Rc::downgrade(bridge), shared, id);
    host.action("plugins.sign", move |arg| {
        let Some(b) = weak.upgrade() else { return };
        if on.borrow().is_some() {
            return;
        }
        let id = arg.field("id").map(text).unwrap_or_default();
        let Some(item) = showable(&b, &id, &None).filter(|item| *ids.borrow() == id && claimable(item)) else {
            return;
        };
        let hash = item.hash().to_string();
        out.set(shown(&b, &id, &None, true, None));
        let (weak, out) = (Rc::downgrade(&b), out.clone());
        engine::get().spawn(async move {
            let result = match mint_review(&id, &hash).await {
                Ok(review) => engine::get().call("SignPlugin", json!({ "id": id, "review": review })).await,
                Err(error) => Err(error),
            };
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

/// Whether an Allow of the sheet `key` may be taken `now`: it's the sheet
/// shown, it has shown `ARMED` at least, and it's this device's.
fn may_allow(seen: &Option<(String, Instant)>, key: &str, elsewhere: bool, now: Instant) -> bool {
    !elsewhere && seen.as_ref().is_some_and(|(shown, since)| shown == key && now.duration_since(*since) >= ARMED)
}

/// Whether "It's mine" may sign `item`: one this device holds, by someone
/// Zeron can't name, as it was made (not changed since you allowed it).
fn claimable(item: &super::Item) -> bool {
    item.status() == "pending" && item.json["author"]["kind"].as_str().is_none_or(|kind| kind == "unknown")
}

/// A review of plugin `id` at `hash`, from the engine: what an Allow or a
/// signing carries, so nothing but this window's host can make one.
async fn mint_review(id: &str, hash: &str) -> Result<String, String> {
    let answer = engine::get().call("ReviewPlugin", json!({ "id": id, "hash": hash })).await?;
    answer["review"].as_str().map(str::to_string).ok_or_else(|| "review: none given".to_string())
}

fn said(error: &str, on: &On) -> String {
    match (error, on) {
        ("offline" | "closed", _) => "Zeron isn't running right now. Try again in a moment.".into(),
        (e, Some((_, name))) if remote::unreachable(e) => format!("Can't reach {name} right now. Try again once it's online."),
        (other, _) => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R27: a plugin's name, version and tools' words can't pass for
    /// Zeron's own on the sheet (a version reading "✓ Zeron", a name
    /// turned around by a direction override, a page of words).
    #[test]
    fn a_plugin_s_words_come_to_the_sheet_bounded_and_plain() {
        let item = json!({
            "name": "Focus\u{202E}nori\u{200B}\n\tTimer",
            "version": "✓ Zeron verified",
            "description": "x".repeat(5000),
            "tools": [{ "name": "count", "description": format!("Count.\u{2066}{}", "y".repeat(900)) }],
            "digest_diff": { "tools_added": [{ "name": "pwn", "description": "z".repeat(900) }], "tools_changed": [] },
        });
        let shown = bounded(&item);
        assert_eq!(shown["name"], "Focusnori Timer");
        assert_eq!(shown["version"], "");
        assert_eq!(shown["description"].as_str().unwrap().chars().count(), WORDS);
        let tool = shown["tools"][0]["description"].as_str().unwrap();
        assert!(tool.starts_with("Count.y") && tool.chars().count() == WORDS, "{tool}");
        assert_eq!(shown["digest_diff"]["tools_added"][0]["description"].as_str().unwrap().chars().count(), WORDS);
        assert_eq!(bounded(&json!({ "version": "0.3.0-beta.1" }))["version"], "0.3.0-beta.1");
        assert_eq!(plain(&"n".repeat(200), NAME).chars().count(), NAME);
    }

    /// R27: Allow is taken only for the sheet shown, once it has shown a
    /// moment, and never for another device's.
    #[test]
    fn allow_waits_for_the_sheet_and_is_this_device_s_alone() {
        let since = Instant::now();
        let seen = Some(("/focus@h1".to_string(), since));
        assert!(!may_allow(&seen, "/focus@h1", false, since + ARMED / 2));
        assert!(may_allow(&seen, "/focus@h1", false, since + ARMED));
        assert!(!may_allow(&seen, "/focus@h2", false, since + ARMED * 2), "changed while shown");
        assert!(!may_allow(&seen, "/focus@h1", true, since + ARMED * 2), "another device's");
        assert!(!may_allow(&None, "/focus@h1", false, since + ARMED * 2));
    }

    #[test]
    fn only_a_stranger_s_unchanged_plugin_can_be_claimed() {
        let item = |status: &str, kind: &str| super::super::Item { json: json!({ "status": status, "author": { "kind": kind } }) };
        assert!(claimable(&item("pending", "unknown")));
        assert!(!claimable(&item("tampered", "unknown")));
        assert!(!claimable(&item("pending", "verified")));
        assert!(!claimable(&item("pending", "you")));
    }
}
