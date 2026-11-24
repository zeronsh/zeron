//! The plugin store, for Settings → Plugins (ui/screens/plugins/store.luau
//! names every call): Browse, a store plugin's page, and publishing from
//! Yours. The engine is the store's one client (crates/engine
//! `plugins/store`: the session, caching, checking a package against the
//! store's record, the pictures it keeps), and agents use the same calls
//! through MCP; this only carries its answers, as src/acp_registry.rs does
//! the registry's. Method by method (crates/rpc `STORE_*`):
//!
//!   plugins.store.search { q?, category?, keyword?, author?, sort?, cursor?, limit?, featured?, verified? }
//!                                     StoreSearch
//!   plugins.store.detail(id | { id, for_zeron })  StorePlugin { id,
//!                                     for_zeron? }: the detail, its first
//!                                     page of versions (`versions_next_cursor`
//!                                     for the rest), `compatible`
//!                                     (`incompatible`: why not), judged for
//!                                     the device shown's Zeron
//!   plugins.store.versions { id, cursor?, limit? }  StoreVersions: a page
//!                                     of its versions, newest first
//!   plugins.store.version { id, version }  StoreVersion: one
//!   plugins.store.categories          StoreCategories
//!   plugins.store.picture { id, version, path }  StoreFile: `{ src }`, the
//!                                     file the engine keeps (a picture only)
//!   plugins.store.icon { id, version, path }     StoreFile: `{ svg }`, its
//!                                     text (an SVG of at most 32 KB, as an
//!                                     installed plugin's icon)
//!   plugins.store.prereleases { id, on, device? }  SetPluginPrereleases
//!   plugins.store.updates { id?, prefer?, device? }  SetPluginUpdates: a
//!                                     plugin's (or with no id the device's)
//!                                     "newest" or "verified" updates
//!   plugins.store.mine                StoreMyPlugins
//!   plugins.store.version_action { id, version, action }  StoreVersionAction
//!   plugins.store.prepare(id)         PreparePluginPublish
//!   plugins.store.publish { id, hash, ticket }  PublishPlugin (`ticket`: the
//!                                     prepare's, which is a handle: the
//!                                     engine's own stays in the host)
//!   plugins.store.tokens()            ListDeployTokens: the account's
//!                                     deploy tokens (never a token itself)
//!   plugins.store.create_token { name, scope, expires? }  CreateDeployToken
//!                                     (`scope`: `{ all = true }` or
//!                                     `{ plugins = { ids } }`), a
//!                                     fresh sign-in begun for it first
//!                                     (StepUp `for: "deploy_token"`, each
//!                                     one a token's); `cancel_step_up`
//!                                     stops waiting for it (and asks the
//!                                     engine's CancelStepUp); its answer
//!                                     `{ ok, token, secret }`, `secret`
//!                                     the token (`zdt_…`) this once
//!   plugins.store.revoke_token { id }  RevokeDeployToken
//!   plugins.store.report { id, reason, text }  StoreReport
//!
//! Each rejects with the engine's words ("offline", "not_in_store", a store
//! word, "<reason>: <words>"), which the page says in its words where it
//! knows them (screens/plugins/words `failure`). Installing is
//! `plugins.install { store, version?, device? }` (mod.rs), and turning a
//! version the store took down back on is `plugins.set_off`.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value as Json, json};
use zgx_script::{ScriptHost, Value};

use super::act_consent::{self, Act};
use super::{json_value, preview};
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

/// The engine, as this asks it: a method and its params, on a device (none:
/// this one). The app's is its IPC (mod.rs `call`, `call_on`); a test's
/// stands in for it.
pub(super) type Engine = Rc<dyn Fn(&'static str, Json, Option<String>) -> Pin<Box<dyn Future<Output = Result<Json, String>>>>>;

pub(super) fn install(host: &ScriptHost, me: std::rc::Weak<super::Bridge>) {
    let asking = me.clone();
    let engine: Engine = Rc::new(move |method, params, device| {
        let me = me.clone();
        Box::pin(async move {
            match device {
                Some(_) => super::call_on(&me, device, method, params).await,
                None => super::call(method, params).await,
            }
        })
    });
    let asker: Asker = {
        let me = asking;
        Rc::new(move |act| Box::pin(super::act_consent::ask(me.clone(), act)))
    };
    install_with(host, engine, asker);
}

/// What you do as yourself, asked in a window of its own (act_consent.rs);
/// a test's stands in for it.
pub(super) type Asker = Rc<dyn Fn(Act) -> Pin<Box<dyn Future<Output = Result<Json, String>>>>>;

/// The store's actions on `engine`. Once a template that isn't Zeron's has
/// run, publishing, taking new installs again and a new token go by `asker`.
pub(super) fn install_with(host: &ScriptHost, engine: Engine, asker: Asker) {
    let e = engine.clone();
    host.async_action("plugins.store.search", move |arg| {
        let e = e.clone();
        async move {
            let mut params = json!({});
            for key in ["q", "category", "keyword", "author", "sort", "cursor"] {
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
            // (Only plugins whose newest version the store has verified.)
            if matches!(arg.field("verified"), Some(Value::Bool(true))) {
                params["verified"] = json!(true);
            }
            ask(&e, "StoreSearch", params).await
        }
    });
    let e = engine.clone();
    host.async_action("plugins.store.detail", move |arg| {
        let e = e.clone();
        async move {
            // `id`, or `{ id, for_zeron }`: judged for another device's Zeron.
            let id = match &arg {
                Value::Str(_) => text(&arg),
                _ => arg.field("id").map(text).unwrap_or_default(),
            };
            let id = store_id(&id).ok_or("not_in_store")?;
            let mut params = json!({ "id": id });
            if let Some(zeron) = arg.field("for_zeron").map(text).filter(|v| version_ok(v)) {
                params["for_zeron"] = json!(zeron);
            }
            ask(&e, "StorePlugin", params).await
        }
    });
    let e = engine.clone();
    host.async_action("plugins.store.versions", move |arg| {
        let e = e.clone();
        async move {
            let id = arg.field("id").map(text).and_then(|i| store_id(&i)).ok_or("not_in_store")?;
            let mut params = json!({ "id": id });
            if let Some(cursor) = arg.field("cursor").map(text).filter(|c| !c.is_empty()) {
                params["cursor"] = json!(cursor);
            }
            if let Some(Value::Num(n)) = arg.field("limit")
                && n.is_finite()
                && *n >= 1.0
            {
                params["limit"] = json!((*n as u64).min(50));
            }
            ask(&e, "StoreVersions", params).await
        }
    });
    let e = engine.clone();
    host.async_action("plugins.store.version", move |arg| {
        let e = e.clone();
        async move {
            let id = arg.field("id").map(text).and_then(|i| store_id(&i)).ok_or("not_in_store")?;
            let version = arg.field("version").map(text).filter(|v| version_ok(v)).ok_or("not_in_store")?;
            ask(&e, "StoreVersion", json!({ "id": id, "version": version })).await
        }
    });
    let e = engine.clone();
    host.async_action("plugins.store.categories", move |_| {
        let e = e.clone();
        async move { ask(&e, "StoreCategories", json!({})).await }
    });
    let e = engine.clone();
    host.async_action("plugins.store.picture", move |arg| {
        let e = e.clone();
        async move {
            let reply = file(&e, &arg).await?;
            let path = reply["file"].as_str().filter(|p| !p.is_empty()).ok_or("not_a_picture")?;
            let kind = reply["type"].as_str().unwrap_or_default();
            if !kind.starts_with("image/") || kind == "image/svg+xml" {
                return Err("not_a_picture".into());
            }
            Ok(Value::record([("src", Value::from(path))]))
        }
    });
    let e = engine.clone();
    host.async_action("plugins.store.icon", move |arg| {
        let e = e.clone();
        async move {
            let reply = file(&e, &arg).await?;
            let path = reply["file"].as_str().filter(|p| !p.is_empty()).map(str::to_owned).ok_or("not_an_icon")?;
            let svg = off_thread(move || read_svg(std::path::Path::new(&path))).await.flatten();
            Ok(svg.map_or(Value::Nil, |svg| Value::record([("svg", Value::from(svg.as_str()))])))
        }
    });
    let e = engine.clone();
    host.async_action("plugins.store.prereleases", move |arg| {
        let e = e.clone();
        async move {
            crate::plugins::preview::refuse("plugins.store.prereleases")?;
            let (id, device) = super::remote::id_and_device(&arg);
            let id = store_id(&id).ok_or("not_in_store")?;
            let on = matches!(arg.field("on"), Some(Value::Bool(true)));
            let reply = e("SetPluginPrereleases", json!({ "id": id, "on": on }), device).await?;
            Ok(json_value(&reply))
        }
    });
    let e = engine.clone();
    host.async_action("plugins.store.updates", move |arg| {
        let e = e.clone();
        async move {
            crate::plugins::preview::refuse("plugins.store.updates")?;
            // (A plugin's own choice, or with no id the device's; `prefer`
            // nil: as the device does, or for the device the default.)
            let id = arg.field("id").map(text).unwrap_or_default();
            let device = super::remote::device_of(&arg);
            let mut params = json!({});
            if !id.is_empty() {
                params["id"] = json!(store_id(&id).ok_or("not_in_store")?);
            }
            params["prefer"] = match arg.field("prefer").map(text).as_deref() {
                Some("newest") => json!("newest"),
                Some("verified") => json!("verified"),
                None | Some("") => Json::Null,
                Some(_) => return Err("invalid".into()),
            };
            let reply = e("SetPluginUpdates", params, device).await?;
            Ok(json_value(&reply))
        }
    });
    let e = engine.clone();
    host.async_action("plugins.store.mine", move |_| {
        let e = e.clone();
        async move {
            // (Your own account, not the catalog: every version of yours,
            // pending and rejected ones with the reviewers' reasons.)
            crate::plugins::preview::refuse("plugins.store.mine")?;
            ask(&e, "StoreMyPlugins", json!({})).await
        }
    });
    let (e, a) = (engine.clone(), asker.clone());
    host.async_action("plugins.store.version_action", move |arg| {
        let (e, a) = (e.clone(), a.clone());
        async move {
            crate::plugins::preview::refuse("plugins.store.version_action")?;
            let id = arg.field("id").map(text).and_then(|i| store_id(&i)).ok_or("not_in_store")?;
            let version = arg.field("version").map(text).filter(|v| version_ok(v)).ok_or("not_in_store")?;
            let action = arg.field("action").map(text).unwrap_or_default();
            // (Its author's: stop new installs, allow them again. Withdrawing
            // is gone: a version is listed as it's published.)
            if !["yank", "unyank"].contains(&action.as_str()) {
                return Err("invalid".into());
            }
            let params = json!({ "id": id, "version": version, "action": action });
            // (Stopping installs takes nothing from anyone; taking them
            // again puts a version back in front of everyone.)
            if action == "unyank" && preview::another_s_ran() {
                let item = act_consent::unyanking("plugin", &id, &version);
                let reply = a(act(item, move || async move { e("StoreVersionAction", params, None).await })).await?;
                answered(&reply)?;
                return Ok(json_value(&reply));
            }
            answer(&e, "StoreVersionAction", params).await
        }
    });
    let e = engine.clone();
    host.async_action("plugins.store.prepare", move |id| {
        let e = e.clone();
        async move {
            crate::plugins::preview::refuse("plugins.store.prepare")?;
            let id = store_id(&text(&id)).ok_or("not_in_store")?;
            let reply = e("PreparePluginPublish", json!({ "id": id }), None).await?;
            Ok(json_value(&keep_prepared(reply)))
        }
    });
    let (e, a) = (engine.clone(), asker.clone());
    host.async_action("plugins.store.publish", move |arg| {
        let (e, a) = (e.clone(), a.clone());
        async move {
            crate::plugins::preview::refuse("plugins.store.publish")?;
            let id = arg.field("id").map(text).and_then(|i| store_id(&i)).ok_or("not_in_store")?;
            let hash = arg.field("hash").map(text).unwrap_or_default();
            // (What's sent, and what's asked, is what the engine said it
            // checked, kept here: never what the page says of it.)
            let was = arg.field("ticket").map(text).and_then(|handle| prepared(&id, &hash, &handle)).ok_or("changed")?;
            let params = json!({ "id": id, "hash": hash, "ticket": was.ticket });
            let send = move || async move {
                // (Good once, whatever comes of it.)
                forget_prepared(&was.handle);
                e("PublishPlugin", params, None).await
            };
            let reply = if preview::another_s_ran() {
                a(act(act_consent::publishing("plugin", &id, &was.name, &was.version), send)).await?
            } else {
                send().await?
            };
            // (A listing refused for a cause it names: the page says which,
            // in its own words, so the refusal comes as the answer.)
            if let Some(refused) = listing_refusal(&reply) {
                return Ok(json_value(&refused));
            }
            answered(&reply)?;
            Ok(json_value(&reply))
        }
    });
    let e = engine.clone();
    host.async_action("plugins.store.tokens", move |_| {
        let e = e.clone();
        async move {
            // (Your own account: your tokens' names, short ids, scopes and
            // when they were last used.)
            crate::plugins::preview::refuse("plugins.store.tokens")?;
            ask(&e, "ListDeployTokens", json!({})).await
        }
    });
    let (e, a) = (engine.clone(), asker.clone());
    host.async_action("plugins.store.create_token", move |arg| {
        let (e, a) = (e.clone(), a.clone());
        async move {
            crate::plugins::preview::refuse("plugins.store.create_token")?;
            // (As it's shown before it's made: one line of what prints.)
            let name = super::consent::plain(&arg.field("name").map(text).unwrap_or_default(), 64);
            if name.is_empty() {
                return Err("invalid_name".into());
            }
            let scope = token_scope(arg.field("scope")).ok_or("invalid_scope")?;
            let mut params = json!({ "name": name, "scope": scope });
            if let Some(Value::Num(at)) = arg.field("expires")
                && at.is_finite()
                && *at > 0.0
            {
                params["expires"] = json!(*at as i64);
            }
            if preview::another_s_ran() {
                let item = act_consent::token(&params);
                let stop = {
                    let e = e.clone();
                    move || {
                        STEP_UP.with(|n| n.set(n.get() + 1));
                        let e = e.clone();
                        crate::engine::get().spawn(async move {
                            let _ = e("CancelStepUp", json!({ "for": "deploy_token" }), None).await;
                        });
                    }
                };
                let undo = {
                    let e = e.clone();
                    move |made: Json| -> act_consent::Answer {
                        let e = e.clone();
                        Box::pin(async move { revoke_made(&e, &made).await })
                    }
                };
                let run = Box::new(move || -> act_consent::Answer { Box::pin(async move { create_token(&e, params, &|| true).await }) });
                let mut reply = a(Act { item, run, stop: Box::new(stop), undo: Some(Box::new(undo)) }).await?;
                // (The window that made it shows it; this one never has it.)
                if let Some(record) = reply.as_object_mut() {
                    record.remove("secret");
                }
                return Ok(json_value(&reply));
            }
            // (The app's own flow. Should another's template start while it
            // waits on the sign-in, it stops there: this window no longer
            // shows a token.)
            let ours = || !preview::another_s_ran();
            let mut reply = create_token(&e, params, &ours).await?;
            if !ours() {
                if let Some(record) = reply.as_object_mut() {
                    record.remove("secret");
                }
                if revoke_made(&e, &reply).await.is_ok() {
                    return Err("cancelled".into());
                }
            }
            Ok(json_value(&reply))
        }
    });
    let e = engine.clone();
    host.async_action("plugins.store.cancel_step_up", move |_| {
        let e = e.clone();
        async move {
            // (No more waiting here, and the engine lets go of a sign-in it
            // began for a token: one finished anyway makes no token unasked.
            // An engine without CancelStepUp yet answers no; nothing else
            // to do then.)
            STEP_UP.with(|n| n.set(n.get() + 1));
            act_consent::give_up();
            let _ = e("CancelStepUp", json!({ "for": "deploy_token" }), None).await;
            Ok(Value::Nil)
        }
    });
    let e = engine.clone();
    host.async_action("plugins.store.revoke_token", move |arg| {
        let e = e.clone();
        async move {
            crate::plugins::preview::refuse("plugins.store.revoke_token")?;
            let id = arg.field("id").map(text).filter(|k| !k.is_empty() && k.len() <= 128).ok_or("not_in_store")?;
            answer(&e, "RevokeDeployToken", json!({ "id": id })).await
        }
    });
    let e = engine;
    host.async_action("plugins.store.report", move |arg| {
        let e = e.clone();
        async move {
            crate::plugins::preview::refuse("plugins.store.report")?;
            let id = arg.field("id").map(text).and_then(|i| store_id(&i)).ok_or("not_in_store")?;
            let reason = arg.field("reason").map(text).unwrap_or_default();
            if !REPORT_REASONS.contains(&reason.as_str()) {
                return Err("invalid".into());
            }
            let words: String = arg.field("text").map(text).unwrap_or_default().chars().take(2000).collect();
            let mut params = json!({ "id": id, "reason": reason });
            if !words.trim().is_empty() {
                params["text"] = json!(words);
            }
            answer(&e, "StoreReport", params).await
        }
    });
}

/// `run` as an act asked in its window, with nothing to stop half-way.
fn act<F: Future<Output = Result<Json, String>> + 'static>(item: Json, run: impl FnOnce() -> F + 'static) -> Act {
    Act { item, run: Box::new(move || Box::pin(run())), stop: Box::new(|| {}), undo: None }
}

/// The token `made` records, revoked: nobody saw it, or will.
async fn revoke_made(e: &Engine, made: &Json) -> Result<Json, String> {
    let id = made["token"]["id"].as_str().filter(|id| !id.is_empty()).ok_or("failed")?;
    let reply = e("RevokeDeployToken", json!({ "id": id }), None).await?;
    answered(&reply)?;
    Ok(reply)
}

/// What the engine checked and gave a ticket for: what publishing with it
/// sends. The ticket stays here: the page has `handle` in its place, which
/// means nothing to the engine.
#[derive(Clone)]
struct Prepared {
    handle: String,
    id: String,
    hash: String,
    ticket: String,
    name: String,
    version: String,
}

thread_local! {
    /// The last few, newest last.
    static PREPARED: RefCell<Vec<Prepared>> = const { RefCell::new(Vec::new()) };
    static NEXT_PREPARED: Cell<u64> = const { Cell::new(0) };
}

const PREPARED_MOST: usize = 16;

/// `reply` as the page has it: its ticket kept here, a handle in its place
/// (none if what it's for isn't all said).
fn keep_prepared(mut reply: Json) -> Json {
    let Some(ticket) = reply.as_object_mut().and_then(|o| o.remove("ticket")) else { return reply };
    let field = |k: &str| reply[k].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    let (Some(ticket), Some(id), Some(hash), Some(version)) = (ticket.as_str().filter(|t| !t.is_empty()), field("id"), field("hash"), field("version")) else {
        return reply;
    };
    let name = reply["name"].as_str().filter(|s| !s.trim().is_empty()).map_or_else(|| id.clone(), |s| s.chars().take(64).collect());
    let n = NEXT_PREPARED.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let handle = format!("prepared-{n}");
    reply["ticket"] = json!(handle);
    PREPARED.with(|p| {
        let mut p = p.borrow_mut();
        p.push(Prepared { handle, id, hash, ticket: ticket.to_owned(), name, version });
        let over = p.len().saturating_sub(PREPARED_MOST);
        p.drain(..over);
    });
    reply
}

fn prepared(id: &str, hash: &str, handle: &str) -> Option<Prepared> {
    PREPARED.with(|p| p.borrow().iter().find(|p| p.handle == handle && p.id == id && p.hash == hash).cloned())
}

fn forget_prepared(handle: &str) {
    PREPARED.with(|p| p.borrow_mut().retain(|p| p.handle != handle));
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

/// `method` asked of this device's engine, its reply for the scripts.
async fn ask(e: &Engine, method: &'static str, params: Json) -> Result<Value, String> {
    let reply = e(method, params, None).await?;
    Ok(json_value(&reply))
}

/// The same, for a method that answers `{ ok: false, error, message }`
/// rather than failing: that answer as its failure.
async fn answer(e: &Engine, method: &'static str, params: Json) -> Result<Value, String> {
    let reply = e(method, params, None).await?;
    answered(&reply)?;
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

/// The causes a refused listing may name (crates/engine plugins/listing.rs
/// `cause::ALL`): the page has a sentence for each.
const LISTING_CAUSES: &[&str] = &["too_long", "not_one_line", "unseen_char", "mark_word", "mark_glyph", "keyword_shape", "too_many", "repeated"];
/// The listing fields those causes are about.
const LISTING_FIELDS: &[&str] = &["name", "description", "keywords"];

/// A publish refused for a listing's field and a cause the page knows,
/// `{ ok = false, error, field, cause, word? }`, each bounded: a known
/// field and cause, the word plain, one line, at most 64 characters. Any
/// other refusal is an error as before (`answered`).
fn listing_refusal(reply: &Json) -> Option<Json> {
    if reply["ok"].as_bool() != Some(false) {
        return None;
    }
    let field = reply["field"].as_str().filter(|f| LISTING_FIELDS.contains(f))?;
    let cause = reply["cause"].as_str().filter(|c| LISTING_CAUSES.contains(c))?;
    let mut out = json!({ "ok": false, "error": "bad_listing", "field": field, "cause": cause });
    if let Some(word) = reply["word"].as_str() {
        let word: String = word.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ").chars().take(64).collect();
        if !word.is_empty() {
            out["word"] = json!(word);
        }
    }
    Some(out)
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
async fn file(e: &Engine, arg: &Value) -> Result<Json, String> {
    let id = arg.field("id").map(text).and_then(|i| store_id(&i)).ok_or("not_in_store")?;
    let version = arg.field("version").map(text).filter(|v| version_ok(v)).ok_or("not_in_store")?;
    let path = arg.field("path").map(text).and_then(|p| package_path(&p)).ok_or("not_in_store")?;
    let reply = e("StoreFile", json!({ "id": id, "version": version, "path": path }), None).await?;
    if !reply["file"].as_str().is_some_and(|f| std::path::Path::new(f).is_absolute()) {
        return Err("not_in_store".into());
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

/// A deploy token's scope as the store takes it: `{ all: true, all_kinds? }`
/// (every plugin of the account's, those it publishes later too, and its
/// templates when `all_kinds` names them), or
/// `{ plugins: [ids] }`, one to fifty store ids, each once (a duplicate
/// refused, never dropped). Anything else is no scope.
fn token_scope(scope: Option<&Value>) -> Option<Json> {
    let scope = scope?;
    if matches!(scope.field("all"), Some(Value::Bool(true))) {
        // (Which kinds: plugins alone unless it says both, said in that
        // order; nothing else is a scope.)
        return match scope.field("all_kinds") {
            None | Some(Value::Nil) => Some(json!({ "all": true })),
            Some(Value::List(kinds)) => {
                let kinds: Vec<String> = kinds.iter().map(text).collect();
                (kinds == ["plugin"] || kinds == ["plugin", "template"]).then(|| json!({ "all": true, "all_kinds": kinds }))
            }
            Some(_) => None,
        };
    }
    let Some(Value::List(items)) = scope.field("plugins") else { return None };
    let mut ids: Vec<String> = Vec::new();
    for item in items.iter() {
        let id = store_id(&text(item))?;
        if ids.contains(&id) {
            return None;
        }
        ids.push(id);
    }
    (1..=50).contains(&ids.len()).then(|| json!({ "plugins": ids }))
}

/// A deploy token made after a fresh sign-in in the browser begun for it
/// (StepUp `for: "deploy_token"`; every token asks, and each sign-in makes
/// one token), asked again each second until it lands, is cancelled, or ten
/// minutes pass. Asked once first, so what makes no token at all (not
/// published yet, not yours) is said before the browser opens. `go_on` is
/// looked at before each ask after the first: `false` gives it up.
async fn create_token(e: &Engine, params: Json, go_on: &dyn Fn() -> bool) -> Result<Json, String> {
    let reply = e("CreateDeployToken", params.clone(), None).await?;
    if reply["ok"].as_bool() != Some(false) || reply["error"].as_str() != Some("step_up_required") {
        answered(&reply)?;
        return Ok(reply);
    }
    let live = STEP_UP.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let sign_in = e("StepUp", json!({ "for": "deploy_token" }), None).await?;
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
        if !go_on() {
            STEP_UP.with(|n| n.set(n.get() + 1));
            let _ = e("CancelStepUp", json!({ "for": "deploy_token" }), None).await;
            return Err("cancelled".into());
        }
        let reply = e("CreateDeployToken", params.clone(), None).await?;
        match reply["error"].as_str() {
            Some("step_up_required") if reply["ok"].as_bool() == Some(false) => {
                if std::time::Instant::now() >= until {
                    return Err("timed_out".into());
                }
            }
            _ => {
                answered(&reply)?;
                return Ok(reply);
            }
        }
    }
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod seam_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every store call that sends, changes or reports anything as you, or
    /// reads your own account (your plugins, your keys), is refused while a
    /// template is only being previewed; the public catalog stays open.
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
        assert!(guarded >= 9, "{guarded}");
        for read in ["plugins.store.search", "plugins.store.detail", "plugins.store.versions", "plugins.store.version", "plugins.store.categories", "plugins.store.picture", "plugins.store.icon"] {
            assert!(!crate::plugins::preview::BLOCKED.contains(&read), "{read} only reads the catalog");
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

    /// A publish refused for a listing's field and cause comes to the page
    /// as its answer, bounded; anything else stays an error.
    #[test]
    fn a_listing_refusal_names_its_field_cause_and_word_bounded() {
        let refused = json!({ "ok": false, "error": "bad_listing", "message": "Its name says \"Verified\"", "field": "name", "cause": "mark_word", "word": "Verified" });
        assert_eq!(listing_refusal(&refused), Some(json!({ "ok": false, "error": "bad_listing", "field": "name", "cause": "mark_word", "word": "Verified" })));
        // The word plain, one line, at most 64 characters.
        let long = json!({ "ok": false, "field": "description", "cause": "unseen_char", "word": format!("U+200B\n{}", "x".repeat(100)) });
        let word = listing_refusal(&long).unwrap()["word"].as_str().unwrap().to_owned();
        assert!(!word.contains('\n') && word.chars().count() == 64, "{word}");
        // None: a field or cause the page doesn't know, no cause, or not a refusal.
        assert_eq!(listing_refusal(&json!({ "ok": false, "field": "icon", "cause": "too_long" })), None);
        assert_eq!(listing_refusal(&json!({ "ok": false, "field": "name", "cause": "claims_zeron" })), None);
        assert_eq!(listing_refusal(&json!({ "ok": false, "error": "not_newer" })), None);
        assert_eq!(listing_refusal(&json!({ "ok": true, "field": "name", "cause": "too_long" })), None);
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
