//! The plugin store, for Settings → Plugins (ui/screens/plugins/store.luau
//! names every call): Browse, a store plugin's page, and publishing from
//! Yours. The engine is the store's one client (crates/engine
//! `plugins/store`: the session, caching, checking a package against the
//! store's record, the pictures it keeps), and agents use the same calls
//! through MCP; this only carries its answers, as src/acp_registry.rs does
//! the registry's. Method by method (crates/rpc `STORE_*`):
//!
//!   plugins.store.search { q?, category?, keyword?, sort?, cursor?, limit?, featured? }
//!                                     StoreSearch
//!   plugins.store.detail(id)          StorePlugin { id }: the detail, its
//!                                     versions, `compatible`
//!                                     (`incompatible_reason`: why not)
//!   plugins.store.categories          StoreCategories
//!   plugins.store.picture { id, version, path }  StoreFile: `{ src }`, the
//!                                     file the engine keeps (a picture only)
//!   plugins.store.icon { id, version, path }     StoreFile: `{ svg }`, its
//!                                     text (an SVG of at most 32 KB, as an
//!                                     installed plugin's icon)
//!   plugins.store.prereleases { id, on, device? }  SetPluginPrereleases
//!   plugins.store.mine                StoreMyPlugins
//!   plugins.store.version_action { id, version, action }  StoreVersionAction
//!   plugins.store.prepare(id)         PreparePluginPublish
//!   plugins.store.publish { id, hash, ticket }  PublishPlugin
//!   plugins.store.keys(id)            ListDeployKeys
//!   plugins.store.create_key { id, name, expires? }  CreateDeployKey, a
//!                                     fresh sign-in first when the store
//!                                     asks for one (StepUp, as deleting the
//!                                     account does); `cancel_step_up` stops
//!                                     waiting for it
//!   plugins.store.revoke_key { id, key }  RevokeDeployKey
//!   plugins.store.report { id, reason, text }  StoreReport
//!
//! Each rejects with the engine's words ("offline", "not_found", a store
//! word, "<reason>: <words>"), which the page says in its words where it
//! knows them (screens/plugins/words `failure`). Installing is
//! `plugins.install { store, version?, device? }` (mod.rs), and turning a
//! version the store took down back on is `plugins.set_off`.

use std::cell::Cell;
use std::time::Duration;

use serde_json::{Value as Json, json};
use zgx_script::{ScriptHost, Value};

use super::json_value;
use crate::auth::text;

/// An icon, at most (an installed plugin's too, mod.rs `icon_svg`).
const ICON_MOST: u64 = 32 * 1024;
/// How long a key's fresh sign-in is waited for, and how often it's looked at.
const STEP_UP_WAIT: Duration = Duration::from_secs(600);
const STEP_UP_EVERY: Duration = Duration::from_secs(1);

thread_local! {
    /// The fresh sign-in being waited for (a new one, or a cancel, moves it).
    static STEP_UP: Cell<u64> = const { Cell::new(0) };
}

pub(super) fn install(host: &ScriptHost, me: std::rc::Weak<super::Bridge>) {
    host.async_action("plugins.store.search", |arg| async move {
        let mut params = json!({});
        for key in ["q", "category", "keyword", "sort", "cursor"] {
            if let Some(v) = arg.field(key).map(text).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
                params[key] = json!(v);
            }
        }
        if let Some(Value::Num(n)) = arg.field("limit")
            && n.is_finite()
            && *n >= 1.0
        {
            params["limit"] = json!((*n as u64).min(50));
        }
        if matches!(arg.field("featured"), Some(Value::Bool(true))) {
            params["featured"] = json!(true);
        }
        engine("StoreSearch", params).await
    });
    host.async_action("plugins.store.detail", |id| async move {
        let id = store_id(&text(&id)).ok_or("not_found")?;
        engine("StorePlugin", json!({ "id": id })).await
    });
    host.async_action("plugins.store.categories", |_| async move { engine("StoreCategories", json!({})).await });
    host.async_action("plugins.store.picture", |arg| async move {
        let reply = file(&arg).await?;
        let path = reply["file"].as_str().filter(|p| !p.is_empty()).ok_or("not_a_picture")?;
        let kind = reply["type"].as_str().unwrap_or_default();
        if !kind.starts_with("image/") || kind == "image/svg+xml" {
            return Err("not_a_picture".into());
        }
        Ok(Value::record([("src", Value::from(path))]))
    });
    host.async_action("plugins.store.icon", |arg| async move {
        let reply = file(&arg).await?;
        let path = reply["file"].as_str().filter(|p| !p.is_empty()).map(str::to_owned).ok_or("not_an_icon")?;
        let svg = off_thread(move || read_svg(std::path::Path::new(&path))).await.flatten();
        Ok(svg.map_or(Value::Nil, |svg| Value::record([("svg", Value::from(svg.as_str()))])))
    });
    host.async_action("plugins.store.prereleases", {
        let me = me.clone();
        move |arg| {
            let me = me.clone();
            async move {
                crate::plugins::preview::refuse("plugins.store.prereleases")?;
                let (id, device) = super::remote::id_and_device(&arg);
                let id = store_id(&id).ok_or("not_found")?;
                let on = matches!(arg.field("on"), Some(Value::Bool(true)));
                let reply = super::call_on(&me, device, "SetPluginPrereleases", json!({ "id": id, "on": on })).await?;
                Ok(json_value(&reply))
            }
        }
    });
    host.async_action("plugins.store.mine", |_| async move { engine("StoreMyPlugins", json!({})).await });
    host.async_action("plugins.store.version_action", |arg| async move {
        crate::plugins::preview::refuse("plugins.store.version_action")?;
        let id = arg.field("id").map(text).and_then(|i| store_id(&i)).ok_or("not_found")?;
        let version = arg.field("version").map(text).filter(|v| version_ok(v)).ok_or("not_found")?;
        let action = arg.field("action").map(text).unwrap_or_default();
        if !["withdraw", "yank", "unyank"].contains(&action.as_str()) {
            return Err("invalid".into());
        }
        engine("StoreVersionAction", json!({ "id": id, "version": version, "action": action })).await
    });
    host.async_action("plugins.store.prepare", |id| async move {
        crate::plugins::preview::refuse("plugins.store.prepare")?;
        let id = store_id(&text(&id)).ok_or("not_found")?;
        engine("PreparePluginPublish", json!({ "id": id })).await
    });
    host.async_action("plugins.store.publish", |arg| async move {
        crate::plugins::preview::refuse("plugins.store.publish")?;
        let id = arg.field("id").map(text).and_then(|i| store_id(&i)).ok_or("not_found")?;
        let hash = arg.field("hash").map(text).unwrap_or_default();
        let ticket = arg.field("ticket").map(text).unwrap_or_default();
        let reply = super::call("PublishPlugin", json!({ "id": id, "hash": hash, "ticket": ticket })).await?;
        answered(&reply)?;
        Ok(json_value(&reply))
    });
    host.async_action("plugins.store.keys", |id| async move {
        let id = store_id(&text(&id)).ok_or("not_found")?;
        engine("ListDeployKeys", json!({ "id": id })).await
    });
    host.async_action("plugins.store.create_key", |arg| async move {
        crate::plugins::preview::refuse("plugins.store.create_key")?;
        let id = arg.field("id").map(text).and_then(|i| store_id(&i)).ok_or("not_found")?;
        let name: String = arg.field("name").map(text).unwrap_or_default().trim().chars().take(64).collect();
        if name.is_empty() {
            return Err("invalid".into());
        }
        let mut params = json!({ "id": id, "name": name });
        if let Some(Value::Num(at)) = arg.field("expires")
            && at.is_finite()
            && *at > 0.0
        {
            params["expires"] = json!(*at as i64);
        }
        create_key(params).await
    });
    host.action("plugins.store.cancel_step_up", |_| {
        STEP_UP.with(|n| n.set(n.get() + 1));
    });
    host.async_action("plugins.store.revoke_key", |arg| async move {
        crate::plugins::preview::refuse("plugins.store.revoke_key")?;
        let id = arg.field("id").map(text).and_then(|i| store_id(&i)).ok_or("not_found")?;
        let key = arg.field("key").map(text).filter(|k| !k.is_empty() && k.len() <= 128).ok_or("not_found")?;
        engine("RevokeDeployKey", json!({ "id": id, "key": key })).await
    });
    host.async_action("plugins.store.report", |arg| async move {
        crate::plugins::preview::refuse("plugins.store.report")?;
        let id = arg.field("id").map(text).and_then(|i| store_id(&i)).ok_or("not_found")?;
        let reason = arg.field("reason").map(text).unwrap_or_default();
        if !REPORT_REASONS.contains(&reason.as_str()) {
            return Err("invalid".into());
        }
        let words: String = arg.field("text").map(text).unwrap_or_default().chars().take(2000).collect();
        engine("StoreReport", json!({ "id": id, "reason": reason, "text": words })).await
    });
}

/// What a report may say is wrong (the plan's amendment 13).
const REPORT_REASONS: &[&str] = &["malware", "misleading", "broken", "copyright", "other"];

/// A store id, as a plugin's id is (crates/engine plugins/manifest.rs
/// `check_id`): `^[a-z0-9][a-z0-9-]{1,39}$`. Checked here only so nothing
/// else is ever sent as one.
pub(crate) fn store_id(id: &str) -> Option<String> {
    let bytes = id.as_bytes();
    let ok = (2..=40).contains(&bytes.len())
        && matches!(bytes[0], b'a'..=b'z' | b'0'..=b'9')
        && bytes[1..].iter().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-'));
    ok.then(|| id.to_owned())
}

async fn engine(method: &str, params: Json) -> Result<Value, String> {
    let reply = super::call(method, params).await?;
    Ok(json_value(&reply))
}

/// An answer that says `{ ok: false, error, message }`: the store's word,
/// with its words when it gave some.
fn answered(reply: &Json) -> Result<(), String> {
    if reply["ok"].as_bool() != Some(false) {
        return Ok(());
    }
    let word = reply["error"].as_str().filter(|w| !w.is_empty()).unwrap_or("failed");
    let message = reply["message"].as_str().unwrap_or_default();
    Err(if message.is_empty() { word.to_string() } else { format!("{word}: {message}") })
}

/// A file of a package, as a README or a listing names it: relative,
/// inside it (no `..`, no `/` first, no scheme, no `\\`, no escapes, no
/// NUL), at most 512 bytes. (The engine checks it against the version's
/// files too.)
fn package_path(path: &str) -> Option<String> {
    if path.is_empty() || path.len() > 512 || path.starts_with('/') || path.contains(['\\', ':', '%', '\0']) {
        return None;
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            part => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// One file of a store version, as the engine keeps it: its answer, its
/// `file` a path here (absolute, or it's no file).
async fn file(arg: &Value) -> Result<Json, String> {
    let id = arg.field("id").map(text).and_then(|i| store_id(&i)).ok_or("not_found")?;
    let version = arg.field("version").map(text).filter(|v| version_ok(v)).ok_or("not_found")?;
    let path = arg.field("path").map(text).and_then(|p| package_path(&p)).ok_or("not_found")?;
    let reply = super::call("StoreFile", json!({ "id": id, "version": version, "path": path })).await?;
    if !reply["file"].as_str().is_some_and(|f| std::path::Path::new(f).is_absolute()) {
        return Err("not_found".into());
    }
    Ok(reply)
}

/// A version as the store names them: SemVer's characters, at most 128.
fn version_ok(v: &str) -> bool {
    !v.is_empty() && v.len() <= 128 && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
}

/// An icon's text, as an installed plugin's is taken (mod.rs `icon_svg`):
/// a file of at most [`ICON_MOST`], UTF-8, that starts as an SVG does.
fn read_svg(path: &std::path::Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > ICON_MOST {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    text.trim_start().starts_with("<svg").then_some(text)
}

/// `work` on a thread of its own, its answer awaited here (None only if it
/// panicked): a file is never read on the window's thread.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, mut rx) = zgx::task::latest();
    std::thread::spawn(move || {
        tx.send(work());
    });
    rx.recv().await
}

/// A deploy key made: at once if the session may, else after a fresh
/// sign-in in the browser (StepUp), asked again each second until it lands,
/// is cancelled, or ten minutes pass.
async fn create_key(params: Json) -> Result<Value, String> {
    let reply = super::call("CreateDeployKey", params.clone()).await?;
    if reply["ok"].as_bool() != Some(false) || reply["error"].as_str() != Some("step_up_required") {
        answered(&reply)?;
        return Ok(json_value(&reply));
    }
    let live = STEP_UP.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let sign_in = super::call("StepUp", json!({})).await?;
    let url = sign_in["url"].as_str().unwrap_or_default();
    if !url.starts_with("https://") {
        return Err("failed".into());
    }
    crate::auth::open_sign_in(url).map_err(|_| "failed".to_string())?;
    let until = std::time::Instant::now() + STEP_UP_WAIT;
    loop {
        zgx::timer::sleep(STEP_UP_EVERY).await;
        if STEP_UP.with(Cell::get) != live {
            return Err("cancelled".into());
        }
        let reply = super::call("CreateDeployKey", params.clone()).await?;
        match reply["error"].as_str() {
            Some("step_up_required") if reply["ok"].as_bool() == Some(false) => {
                if std::time::Instant::now() >= until {
                    return Err("timed_out".into());
                }
            }
            _ => {
                answered(&reply)?;
                return Ok(json_value(&reply));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every store call that sends, changes or reports anything as you is
    /// refused while a template is only being previewed.
    #[test]
    fn a_preview_can_t_act_as_you_in_the_store() {
        let source = include_str!("catalog.rs");
        let mut guarded = 0;
        for line in source.lines() {
            if let Some(rest) = line.split("preview::refuse(\"").nth(1) {
                let name = rest.split('"').next().unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                assert!(crate::plugins::preview::BLOCKED.contains(&name), "{name} isn't blocked in a preview");
                guarded += 1;
            }
        }
        assert!(guarded >= 7, "{guarded}");
        for read in ["plugins.store.search", "plugins.store.detail", "plugins.store.mine", "plugins.store.keys"] {
            assert!(!crate::plugins::preview::BLOCKED.contains(&read), "{read} only reads");
        }
    }

    #[test]
    fn a_package_file_is_inside_its_package() {
        assert_eq!(package_path("shots/a.png").as_deref(), Some("shots/a.png"));
        assert_eq!(package_path("./shots//a.png").as_deref(), Some("shots/a.png"));
        for bad in ["", "/etc/passwd", "../x.png", "shots/../../x.png", "%2e%2e/x.png", "https://e.com/a.png", "file:///a", "C:\\a.png", "a\0.png"] {
            assert_eq!(package_path(bad), None, "{bad:?}");
        }
        assert!(version_ok("1.2.3-beta.1+b"));
        assert!(!version_ok("1.2.3/../x"));
    }

    #[test]
    fn only_a_store_id_is_one() {
        assert_eq!(store_id("focus-timer").as_deref(), Some("focus-timer"));
        for bad in ["", "a", "-a", "A1", "a_b", "a/b", "../x", &"a".repeat(41), "fócus"] {
            assert_eq!(store_id(bad), None, "{bad}");
        }
    }

    #[test]
    fn an_answer_that_says_no_is_the_store_s_word() {
        assert!(answered(&json!({ "ok": true })).is_ok());
        assert!(answered(&json!({ "id": "x" })).is_ok());
        assert_eq!(answered(&json!({ "ok": false, "error": "not_newer", "message": "1.0.0 is older" })).unwrap_err(), "not_newer: 1.0.0 is older");
        assert_eq!(answered(&json!({ "ok": false, "error": "changed" })).unwrap_err(), "changed");
        assert_eq!(answered(&json!({ "ok": false })).unwrap_err(), "failed");
    }

    #[test]
    fn an_icon_is_an_svg_file_of_its_size() {
        let dir = std::env::temp_dir().join(format!("zeron-store-icons-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ok = dir.join("ok.svg");
        std::fs::write(&ok, "  <svg viewBox=\"0 0 1 1\"/>").unwrap();
        assert!(read_svg(&ok).is_some());
        let html = dir.join("html.svg");
        std::fs::write(&html, "<html><svg/></html>").unwrap();
        assert!(read_svg(&html).is_none());
        let big = dir.join("big.svg");
        std::fs::write(&big, format!("<svg>{}</svg>", "x".repeat(ICON_MOST as usize))).unwrap();
        assert!(read_svg(&big).is_none());
        assert!(read_svg(&dir.join("none.svg")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
