//! The store's IPC methods, and installing and updating plugins from it:
//! the IO around `store::verify`, `store::publish` and the client.
//!
//! Who may do what (`zeron_rpc::local_only`, `agent_allowed`): every store
//! method is this device's own (never a peer's through the relay). Agents
//! may search and read the store, install from it (it lands waiting for the
//! person, as any install does) and check one of the person's plugins
//! against its rules; submitting a plugin, acting on its versions and its
//! deploy keys are the person's, in Zeron's window or the CLI. Submitting
//! also needs the ticket `PreparePluginPublish` gives this device's app or
//! CLI for the plugin as checked (`reviews::Kind::Publish`), so what's sent
//! is what the person saw checked.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use zeron_rpc::{RpcError, RpcReply, methods, parse_params};

use super::client::{self, Caller, Installed as Asked, Offer, PackageRecord, Search, StoreClient, UpdatesAsk};
use super::deploy_key::{DeployKey, Scope};
use super::notices;
use super::publish;
use crate::plugins::install::{self, Found, STORE, Source, failure};
use crate::plugins::listing::{self, CATEGORIES};
use crate::plugins::status::{Blocked, UpdatePreference};
use crate::plugins::version::{self, Version};
use crate::plugins::{Plugins, capability, failed, lock, manifest, now_secs, package, reviews, signing};

/// The most one store file fetched for showing may be.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Versions of a plugin whose files stay cached.
const CACHED_VERSIONS: usize = 3;
/// The most every store file cached here comes to: past it, the oldest
/// versions' go.
const MAX_CACHE_BYTES: u64 = 256 * 1024 * 1024;
/// The most pages of the take-down feed one check reads.
const TAKEDOWN_PAGES: usize = 20;
/// How long an account with no plugins waits between looks: an hour, or a
/// local run's shorter setting (`ZERON_TAKEDOWN_EVERY_SECS`), never longer.
pub(in crate::plugins) fn publishes_idle(local: Option<std::time::Duration>) -> std::time::Duration {
    local.map_or(PUBLISHES_IDLE, |every| every.min(PUBLISHES_IDLE))
}

/// At most this many pages of versions (50 each) are looked through for a
/// verified version to offer instead.
const ALTERNATIVE_PAGES: usize = 4;
/// How often an account with no plugins has its publishes looked over (one
/// with plugins: at each take-down check; and a publish from here looks at
/// the next).
const PUBLISHES_IDLE: std::time::Duration = std::time::Duration::from_secs(60 * 60);
/// The most plugins one update check asks about (the edge's limit).
pub const MAX_UPDATE_PLUGINS: usize = 200;
/// What a report may say it's about (contract amendment 13).
pub const REPORT_REASONS: &[&str] = &["malware", "misleading", "broken", "copyright", "other"];
pub const CACHE_DIR: &str = "store-cache";

/// The store's methods (`InstallPlugin {store}` and the update checks are
/// lifecycle's, as every source's are).
pub fn handles(method: &str) -> bool {
    matches!(
        method,
        methods::STORE_SEARCH
            | methods::STORE_PLUGIN
            | methods::STORE_VERSIONS
            | methods::STORE_VERSION
            | methods::STORE_CATEGORIES
            | methods::STORE_STATS
            | methods::STORE_FILE
            | methods::SET_PLUGIN_PRERELEASES
            | methods::SET_PLUGIN_UPDATES
            | methods::PREPARE_PLUGIN_PUBLISH
            | methods::PUBLISH_PLUGIN
            | methods::STORE_MY_PLUGINS
            | methods::STORE_VERSION_ACTION
            | methods::LIST_DEPLOY_KEYS
            | methods::CREATE_DEPLOY_KEY
            | methods::REVOKE_DEPLOY_KEY
            | methods::STORE_REPORT
            | methods::ACK_STORE_NOTICES
    )
}

/// Where the store is, and the session that speaks to it.
#[derive(Clone)]
pub(in crate::plugins) struct StoreHandle {
    pub client: StoreClient,
    pub session: Option<Arc<dyn zeron_rpc::TokenSource>>,
}

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
struct PluginParams {
    id: String,
    /// Judged for this version of Zeron (another device's), not this one's.
    #[serde(default)]
    for_zeron: Option<String>,
}

#[derive(Deserialize)]
struct VersionsParams {
    id: String,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Deserialize)]
struct VersionParams {
    id: String,
    version: String,
}

#[derive(Deserialize)]
struct UpdatesParams {
    /// One plugin's (none: the device's).
    #[serde(default)]
    id: Option<String>,
    /// `newest`, `verified`, or null (as the device does / the default).
    #[serde(default)]
    prefer: Option<String>,
}

#[derive(Deserialize)]
struct ReportParams {
    id: String,
    reason: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct FileParams {
    id: String,
    version: String,
    path: String,
}

#[derive(Deserialize)]
struct PrereleaseParams {
    id: String,
    on: bool,
}

#[derive(Deserialize)]
struct PublishParams {
    id: String,
    hash: String,
    #[serde(default)]
    ticket: Option<String>,
}

#[derive(Deserialize)]
struct VersionActionParams {
    id: String,
    version: String,
    action: String,
}

#[derive(Deserialize)]
struct CreateKeyParams {
    name: String,
    /// `{all: true}` or `{plugins: [id…]}` (amendment 44).
    scope: ScopeParams,
    /// Epoch ms.
    #[serde(default)]
    expires: Option<i64>,
}

#[derive(Deserialize)]
struct ScopeParams {
    #[serde(default)]
    all: bool,
    #[serde(default)]
    plugins: Option<Vec<String>>,
}

impl ScopeParams {
    fn scope(&self) -> Result<Scope, String> {
        match (self.all, &self.plugins) {
            (true, None) => Ok(Scope::All),
            (false, Some(ids)) => Scope::plugins(ids),
            _ => Err("A key's scope is all of your plugins ({all: true}) or the ones it names ({plugins: [...]})".into()),
        }
    }
}

#[derive(Deserialize)]
struct AckNoticesParams {
    ids: Vec<String>,
}

#[derive(Deserialize)]
struct RevokeKeyParams {
    key: String,
}

/// A store plugin installed here, as the update check asks about it.
struct Candidate {
    id: String,
    name: String,
    version: String,
    hash: String,
    /// Its owner's stable id in the store.
    publisher_id: String,
    permissions: Vec<String>,
    allowed: Value,
    prerelease: bool,
    /// The key that signed the version installed.
    key: Option<String>,
    /// The version installed was verified (its record's, at install).
    verified: bool,
    /// Which update this device stages for it.
    prefer: UpdatePreference,
}

/// `{ok: false, error, message}` from a failure's `"<word>: <words>"`.
fn not_ok(error: &str) -> Value {
    let word = install::reason_of(error).unwrap_or("failed");
    let message = error.strip_prefix(&format!("{word}: ")).unwrap_or(error);
    json!({ "ok": false, "error": word, "message": message })
}

/// [`not_ok`] with a listing problem's fields (`field`, `cause`, `word`),
/// those that are there: for an app that says it in its own words.
fn refused(error: &str, fields: &Value) -> Value {
    let mut value = not_ok(error);
    for key in ["field", "cause", "word"] {
        if let Some(text) = fields[key].as_str() {
            value[key] = json!(text);
        }
    }
    value
}

/// Characters that change how the text around them reads (direction
/// overrides, invisible joiners): never kept from the store's words.
fn invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' | '\u{061C}')
}

/// A store card or plugin page as this device passes it on: its one-line
/// words (name, summary, description, license, the author's username,
/// categories, keywords) without control or format characters, and its
/// versions' notes without any but line breaks. The edge refuses them in a
/// listing; a hostile or broken answer still can't draw them in a dialog.
fn clean_card(card: &mut Value) {
    let one_line = |text: &str| text.chars().filter(|c| !listing::unseen_char(*c)).collect::<String>();
    let Some(object) = card.as_object_mut() else { return };
    for key in ["name", "summary", "description", "license"] {
        if let Some(Value::String(text)) = object.get_mut(key) {
            *text = one_line(text);
        }
    }
    if let Some(Value::String(name)) = object.get_mut("author").and_then(|a| a.get_mut("username")) {
        *name = one_line(name);
    }
    for key in ["categories", "keywords"] {
        for item in object.get_mut(key).and_then(Value::as_array_mut).into_iter().flatten() {
            if let Value::String(text) = item {
                *text = one_line(text);
            }
        }
    }
    for version in object.get_mut("versions").and_then(Value::as_array_mut).into_iter().flatten() {
        if let Some(Value::String(text)) = version.get_mut("notes") {
            *text = text.chars().filter(|c| *c == '\n' || !listing::unseen_char(*c)).collect();
        }
    }
}

/// Words from the store, made safe to keep and show: one line, no control
/// or direction-changing characters, not too long.
fn plain(text: &str, max: usize) -> String {
    text.chars()
        .filter(|c| !invisible(*c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A version's notes (markdown) from the store: [`plain`], its lines kept.
fn notes(text: &str, max: usize) -> String {
    text.chars()
        .filter(|c| !invisible(*c) && (*c == '\n' || !c.is_control()))
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Whether `a` and `b` name the same version (strict precedence, or the
/// same text when either isn't strict).
fn same_version(a: &str, b: &str) -> bool {
    match (Version::parse(a), Version::parse(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Whether `candidate` is later than `installed`: by strict precedence (a
/// store version always is one), else as sideloading compares.
fn later(candidate: &str, installed: &str) -> bool {
    install::newer_by_precedence(candidate, installed)
}

/// A store file's media type, by its name.
fn media_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("md") => "text/markdown",
        Some("txt") => "text/plain",
        _ => "application/octet-stream",
    }
}

impl Plugins {
    /// Where the store is (the edge the engine signs in against, or a
    /// stand-in), and the session its signed-in calls carry (none: public
    /// reads only).
    pub fn set_store(&self, client: StoreClient, session: Option<Arc<dyn zeron_rpc::TokenSource>>) {
        *lock(&self.inner.store) = Some(StoreHandle { client, session });
    }

    fn store_handle(&self) -> Result<StoreHandle, RpcError> {
        lock(&self.inner.store).clone().ok_or_else(|| failed(failure(client::reason::STORE_UNREACHABLE, "This Zeron has no store to reach")))
    }

    /// The store, and the caller to read it as: the session when signed in
    /// (your own pending versions show), else anonymous.
    fn store_reader(&self) -> Result<(StoreClient, Caller), RpcError> {
        let handle = self.store_handle()?;
        let caller = handle.session.map_or(Caller::Anonymous, Caller::Session);
        Ok((handle.client, caller))
    }

    /// The store, as the signed-in person: `Err` (`signed_out`) when nobody is.
    async fn store_as_person(&self) -> Result<(StoreClient, Caller), String> {
        let handle = self.store_handle().map_err(|e| e.to_string())?;
        let signed_out = || failure("signed_out", "Sign in to Zeron first");
        let session = handle.session.ok_or_else(signed_out)?;
        session.token().await.map_err(|_| signed_out())?;
        Ok((handle.client, Caller::Session(session)))
    }

    pub(in crate::plugins) async fn store_method(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
        match method {
            methods::STORE_SEARCH => {
                let search: Search = parse_params(params)?;
                let (client, caller) = self.store_reader()?;
                let mut reply = client.search(&search, &caller).await.map_err(failed)?;
                self.rescan().await;
                if let Some(items) = reply.get_mut("items").and_then(Value::as_array_mut) {
                    for item in items {
                        clean_card(item);
                        if let Some(installed) = item["id"].as_str().and_then(|id| self.installed_here(id)) {
                            item["installed"] = installed;
                        }
                    }
                }
                RpcReply::value(&reply)
            }
            methods::STORE_PLUGIN => {
                let p: PluginParams = parse_params(params)?;
                manifest::check_id(&p.id).map_err(failed)?;
                let (client, caller) = self.store_reader()?;
                let (mut reply, _) = client.plugin(&p.id, &caller).await.map_err(failed)?;
                clean_card(&mut reply);
                let zeron = match p.for_zeron.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                    Some(v) => Version::parse(v).map_err(failed)?,
                    None => version::zeron(),
                };
                let range = reply["engines"]["zeron"].as_str().map(str::to_owned);
                match version::accepts(range.as_deref(), &zeron) {
                    Ok(()) => reply["compatible"] = json!(true),
                    Err(why) => {
                        reply["compatible"] = json!(false);
                        reply["incompatible"] = json!(why);
                    }
                }
                self.rescan().await;
                if let Some(installed) = self.installed_here(&p.id) {
                    reply["installed"] = installed;
                }
                RpcReply::value(&reply)
            }
            methods::STORE_VERSIONS => {
                let p: VersionsParams = parse_params(params)?;
                manifest::check_id(&p.id).map_err(failed)?;
                let (client, caller) = self.store_reader()?;
                let mut reply = client.versions(&p.id, p.cursor.as_deref(), p.limit, &caller).await.map_err(failed)?;
                clean_card(&mut reply);
                RpcReply::value(&reply)
            }
            methods::STORE_VERSION => {
                let p: VersionParams = parse_params(params)?;
                manifest::check_id(&p.id).map_err(failed)?;
                let (client, caller) = self.store_reader()?;
                let (mut reply, _) = client.version(&p.id, p.version.trim(), &caller).await.map_err(failed)?;
                if let Some(text) = reply.get_mut("notes").and_then(|n| n.as_str().map(str::to_owned)) {
                    reply["notes"] = json!(notes(&text, listing::MAX_NOTES_BYTES));
                }
                RpcReply::value(&reply)
            }
            methods::STORE_STATS => {
                let (client, _) = self.store_reader()?;
                RpcReply::value(&client.stats().await.map_err(failed)?)
            }
            methods::SET_PLUGIN_UPDATES => {
                let p: UpdatesParams = parse_params(params)?;
                let prefer = match p.prefer.as_deref().map(str::trim).filter(|w| !w.is_empty()) {
                    None => None,
                    Some(word) => Some(UpdatePreference::parse(word).ok_or_else(|| failed("Updates are newest or verified"))?),
                };
                if let Some(id) = &p.id {
                    self.rescan().await;
                    let entry = self.entry(id).ok_or_else(|| failed(format!("There's no plugin {id}")))?;
                    if entry.store.is_none() {
                        return Err(failed("Only a plugin installed from the store has store updates to choose"));
                    }
                }
                self.update_state(|state| match (&p.id, prefer) {
                    (Some(id), Some(prefer)) => {
                        state.store_updates_for.insert(id.clone(), prefer);
                    }
                    (Some(id), None) => {
                        state.store_updates_for.remove(id);
                    }
                    (None, prefer) => state.store_updates = prefer,
                })?;
                let state = lock(&self.inner.state).clone();
                let device = state.store_updates.unwrap_or(crate::plugins::status::DEFAULT_UPDATES);
                let effective = p.id.as_deref().map_or(device, |id| state.updates_for(id));
                RpcReply::value(&json!({ "prefer": effective.word(), "device": device.word() }))
            }
            methods::STORE_CATEGORIES => {
                // Offline, the fixed list this Zeron knows.
                let reply = match self.store_reader() {
                    Ok((client, _)) => client.categories().await.ok(),
                    Err(_) => None,
                };
                RpcReply::value(&reply.unwrap_or_else(|| {
                    json!({ "categories": CATEGORIES.iter().map(|(id, name)| json!({ "id": id, "name": name })).collect::<Vec<_>>() })
                }))
            }
            methods::STORE_FILE => {
                let p: FileParams = parse_params(params)?;
                RpcReply::value(&self.store_file(p).await.map_err(failed)?)
            }
            methods::SET_PLUGIN_PRERELEASES => {
                let p: PrereleaseParams = parse_params(params)?;
                self.rescan().await;
                let entry = self.entry(&p.id).ok_or_else(|| failed(format!("There's no plugin {}", p.id)))?;
                if entry.store.is_none() {
                    return Err(failed("Only a plugin installed from the store has pre-releases to take"));
                }
                self.update_state(|state| {
                    if p.on {
                        state.prerelease.insert(p.id.clone());
                    } else {
                        state.prerelease.remove(&p.id);
                    }
                })?;
                self.rescan().await;
                self.item(&p.id)
            }
            methods::PREPARE_PLUGIN_PUBLISH => {
                let p: IdParams = parse_params(params)?;
                let dir = self.own_dir(&p.id).await?;
                let mut prepared = tokio::task::spawn_blocking(move || publish::read(&dir))
                    .await
                    .map_err(|_| failed("Couldn't read it"))?
                    .map_err(failed)?;
                // Newer than what the store has, as it said a moment ago (it
                // has the last word: offline or signed out, it isn't asked).
                let versions = match self.store_as_person().await {
                    Ok((client, caller)) => client.my_plugins(&caller).await.ok().map(|mine| {
                        mine["plugins"].as_array().into_iter().flatten().find(|plugin| plugin["id"] == p.id.as_str()).map(|plugin| plugin["versions"].clone())
                    }),
                    Err(_) => None,
                };
                if let Some(Some(Value::Array(versions))) = &versions
                    && let Some(problem) = publish::newer_problem(&prepared.manifest.version, versions)
                {
                    prepared.problems.push(problem);
                }
                let mut reply = prepared.shown();
                reply["store_checked"] = json!(versions.is_some());
                if versions.is_none() {
                    reply["note"] = json!("Couldn't ask the store which versions it has: the store has the last word on whether this one is newer");
                }
                // The ticket to submit it as checked: this device's app or
                // CLI alone (an agent checks, the person submits).
                if prepared.ok() && zeron_rpc::caller().is_local() {
                    let ticket = lock(&self.inner.reviews).mint(reviews::Kind::Publish, reviews::Origin::Local, &p.id, &prepared.hash)?;
                    reply["ticket"] = json!(ticket);
                }
                RpcReply::value(&reply)
            }
            methods::PUBLISH_PLUGIN => {
                let p: PublishParams = parse_params(params)?;
                if !zeron_rpc::caller().is_local() {
                    return Err(reviews::needed("Publishing a plugin"));
                }
                lock(&self.inner.reviews).redeem(reviews::Kind::Publish, &reviews::Origin::Local, &p.id, &p.hash, p.ticket.as_deref())?;
                RpcReply::value(&self.publish(&p.id, &p.hash).await)
            }
            methods::STORE_MY_PLUGINS => {
                let (client, caller) = self.store_as_person().await.map_err(failed)?;
                let mut reply = client.my_plugins(&caller).await.map_err(failed)?;
                for plugin in reply.get_mut("plugins").and_then(Value::as_array_mut).into_iter().flatten() {
                    clean_card(plugin);
                }
                RpcReply::value(&reply)
            }
            methods::STORE_VERSION_ACTION => {
                let p: VersionActionParams = parse_params(params)?;
                manifest::check_id(&p.id).map_err(failed)?;
                Version::parse(&p.version).map_err(failed)?;
                // (What an author does to a version: amendment 45.)
                if !matches!(p.action.as_str(), "yank" | "unyank") {
                    return Err(failed("The action is yank or unyank"));
                }
                let (client, caller) = self.store_as_person().await.map_err(failed)?;
                let reply = client.version_action(&p.id, &p.version, &p.action, &caller).await;
                RpcReply::value(&match reply {
                    Ok(mut value) => {
                        value["ok"] = json!(true);
                        value
                    }
                    Err(error) => not_ok(&error),
                })
            }
            methods::LIST_DEPLOY_KEYS => {
                let (client, caller) = self.store_as_person().await.map_err(failed)?;
                RpcReply::value(&client.keys(&caller).await.map_err(failed)?)
            }
            methods::CREATE_DEPLOY_KEY => {
                let p: CreateKeyParams = parse_params(params)?;
                RpcReply::value(&self.create_deploy_key(p).await)
            }
            methods::STORE_REPORT => {
                let p: ReportParams = parse_params(params)?;
                manifest::check_id(&p.id).map_err(failed)?;
                if !REPORT_REASONS.contains(&p.reason.as_str()) {
                    return Err(failed(format!("The reason is one of {}", REPORT_REASONS.join(", "))));
                }
                let text = p.text.as_deref().map(|t| notes(t, 2000)).filter(|t| !t.is_empty());
                let (client, caller) = self.store_as_person().await.map_err(failed)?;
                RpcReply::value(&match client.report(&p.id, &p.reason, text.as_deref(), &caller).await {
                    Ok(mut value) => {
                        value["ok"] = json!(true);
                        value
                    }
                    Err(error) => not_ok(&error),
                })
            }
            methods::ACK_STORE_NOTICES => {
                let p: AckNoticesParams = parse_params(params)?;
                self.update_state(|state| state.publish_notices.retain(|n| !p.ids.contains(&n.id)))?;
                self.publish_list();
                RpcReply::value(&json!({}))
            }
            methods::REVOKE_DEPLOY_KEY => {
                let p: RevokeKeyParams = parse_params(params)?;
                if !super::deploy_key::valid_public(p.key.trim()) {
                    return Err(failed("That isn't a deploy key: give its key as `zeron plugin key list` shows it"));
                }
                let (client, caller) = self.store_as_person().await.map_err(failed)?;
                client.revoke_key(p.key.trim(), &caller).await.map_err(failed)?;
                RpcReply::value(&json!({ "revoked": p.key.trim() }))
            }
            _ => Err(RpcError::UnknownMethod(method.to_owned())),
        }
    }

    /// What the store's pages say of a plugin installed here.
    fn installed_here(&self, id: &str) -> Option<Value> {
        let entry = self.entry(id)?;
        let mut installed = json!({
            "version": entry.item.version,
            "hash": entry.item.hash,
            "status": entry.item.status,
            "store": entry.store.is_some(),
        });
        if let Some(store) = &entry.store {
            installed["verified"] = json!(store.verified);
            installed["updates"] = json!(lock(&self.inner.state).updates_for(id).word());
        }
        if let Some(update) = &entry.item.update {
            installed["update"] = update.clone();
        }
        if let Some(blocked) = &entry.item.blocked {
            installed["blocked"] = blocked.clone();
        }
        Some(installed)
    }

    /// The folder of `id`, one of your own plugins here.
    async fn own_dir(&self, id: &str) -> Result<PathBuf, RpcError> {
        self.rescan().await;
        let entry = self.entry(id).ok_or_else(|| failed(format!("There's no plugin {id}")))?;
        if !entry.item.own || entry.item.builtin {
            return Err(failed(
                "Only a plugin you're making on this device can be published from here (a folder anywhere publishes with `zeron plugin publish <folder>` and a deploy key)",
            ));
        }
        Ok(PathBuf::from(&entry.item.dir))
    }

    /// Your plugin `id`, as checked (`hash`), signed with this device's key
    /// and sent to the store with your session: the store's answer.
    async fn publish(&self, id: &str, hash: &str) -> Value {
        let dir = match self.own_dir(id).await {
            Ok(dir) => dir,
            Err(error) => return not_ok(&error.to_string()),
        };
        let (client, caller) = match self.store_as_person().await {
            Ok(store) => store,
            Err(error) => return not_ok(&error),
        };
        // The store takes packages signed by keys bound to your account:
        // this device's, registered now if it isn't yet.
        self.register_key().await;
        let this = self.clone();
        let (id_owned, hash) = (id.to_owned(), hash.to_owned());
        let packed = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, (String, Value)> {
            // Re-signed as it is now, then read as it'll be sent.
            this.auto_resign(&id_owned);
            let prepared = publish::read(&dir).map_err(|e| (e, Value::Null))?;
            if prepared.hash != hash {
                return Err((failure("changed", "It changed since it was checked: check it again"), Value::Null));
            }
            if !prepared.ok() {
                let first = &prepared.problems[0];
                let fields = json!({ "field": first.field, "cause": first.cause, "word": first.word });
                return Err((failure(first.rule, publish::lines(&prepared.problems).join("; ")), fields));
            }
            let key = signing::load_or_create_key(&this.inner.data_dir).map_err(|e| (e, Value::Null))?;
            prepared.pack(&key).map_err(|e| (e, Value::Null))
        })
        .await;
        let raw = match packed {
            Ok(Ok(raw)) => raw,
            Ok(Err((error, fields))) => return refused(&error, &fields),
            Err(_) => return not_ok("Couldn't pack it"),
        };
        match client.publish(id, raw, &caller).await {
            Ok(mut reply) => {
                reply["ok"] = json!(true);
                // The account has a plugin now: its publishes are looked over
                // at the next check, not an idle hour on.
                *lock(&self.inner.publishes_checked) = None;
                // (Live at once: its page on the store's website.)
                reply["url"] = json!(client.page_url(id));
                reply
            }
            Err((error, fields)) => refused(&error, &fields),
        }
    }

    /// A deploy key for the account (amendment 44), its scope fixed now,
    /// made here and registered at the edge with a step-up begun for it and
    /// its proof: its secret, shown this once.
    async fn create_deploy_key(&self, p: CreateKeyParams) -> Value {
        let scope = match p.scope.scope() {
            Ok(scope) => scope,
            Err(error) => return not_ok(&failure("invalid_scope", error)),
        };
        let name = plain(&p.name, 64);
        if name.is_empty() {
            return not_ok(&failure("invalid", "Give the key a name (where it's used, like \"GitHub Actions\")"));
        }
        let (client, caller) = match self.store_as_person().await {
            Ok(store) => store,
            Err(error) => return not_ok(&error),
        };
        // A key publishes plugins you have in the store already (a first
        // version comes from a signed-in device): said here, by id, before
        // the step-up; and the account's publisher id, which its proof binds.
        let mine = match client.my_plugins(&caller).await {
            Ok(mine) => mine,
            Err(error) => return not_ok(&error),
        };
        let owned: Vec<&str> = mine["plugins"].as_array().into_iter().flatten().filter_map(|plugin| plugin["id"].as_str()).collect();
        if let Scope::Plugins(ids) = &scope {
            let missing: Vec<&str> = ids.iter().map(String::as_str).filter(|id| !owned.contains(id)).collect();
            if !missing.is_empty() {
                return not_owner(&missing);
            }
        }
        let Some(publisher_id) = own_publisher_id(&mine) else {
            return not_ok(&failure(
                "not_published",
                "You have no plugin in the store yet: publish one from this device (`zeron plugin publish <id>`), then make a deploy key",
            ));
        };
        // A step-up begun for this (`StepUp {for: "deploy_key"}`), used up
        // here: a sign-in's ticket, or one already spent, isn't one.
        let step_up = lock(&self.inner.deploy_key_step_up).clone().and_then(|f| f());
        let Some(step_up) = step_up else {
            return not_ok(&failure("step_up_required", "Making a deploy key needs a fresh sign-in for it"));
        };
        let key = match DeployKey::generate() {
            Ok(key) => key,
            Err(error) => return not_ok(&error),
        };
        let proof = key.proof(&publisher_id, p.expires, &scope);
        let mut body = json!({ "key": key.public(), "name": name, "scope": scope.to_value(), "proof": proof, "stepUp": step_up });
        if let Some(expires) = p.expires {
            body["expires"] = json!(expires);
        }
        match client.add_key(body, &caller).await {
            Ok(registered) => {
                tracing::info!(scope = %scope.text(), key = %key.public(), "store: a deploy key was made");
                json!({ "ok": true, "key": registered, "secret": key.secret() })
            }
            Err(error) => not_ok(&error),
        }
    }

    /// One file of a store version, from this device's cache or fetched
    /// into it.
    async fn store_file(&self, p: FileParams) -> Result<Value, String> {
        manifest::check_id(&p.id)?;
        Version::parse(&p.version)?;
        if !package::plain_path(&p.path) || p.path == manifest::MANIFEST_FILE {
            return Err("That isn't a file in a plugin".into());
        }
        let cache = self.inner.data_dir.join(CACHE_DIR).join(&p.id);
        // Named by the version's exact text, hashed: `1.0.0-RC` and
        // `1.0.0-rc` are two versions, also on a disk that ignores case.
        let version_dir = cache.join(&signing::sha256_hex(p.version.as_bytes())[..32]);
        let path = zeron_plugin_guard::path::join(&version_dir, &p.path)?;
        let shown = |path: &Path, bytes: u64| json!({ "file": path.display().to_string(), "type": media_type(&p.path), "bytes": bytes });
        if let Ok(meta) = std::fs::metadata(&path)
            && meta.is_file()
        {
            return Ok(shown(&path, meta.len()));
        }
        let (client, caller) = self.store_reader().map_err(|e| e.to_string())?;
        let bytes = client.file(&p.id, &p.version, &p.path, &caller, MAX_FILE_BYTES).await?;
        let written = path.clone();
        let len = bytes.len() as u64;
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let parent = written.parent().ok_or("Couldn't keep it")?;
            std::fs::create_dir_all(parent).map_err(|_| "Couldn't keep it".to_string())?;
            let temp = parent.join(format!(".part-{}", crate::new_id()));
            std::fs::write(&temp, &bytes).and_then(|()| std::fs::rename(&temp, &written)).map_err(|_| {
                let _ = std::fs::remove_file(&temp);
                "Couldn't keep it".to_string()
            })?;
            prune_cache(&cache);
            Ok(())
        })
        .await
        .map_err(|_| "Couldn't keep it".to_string())??;
        Ok(shown(&path, len))
    }

    // ── Installing and updating ─────────────────────────────────────────

    /// Install `id` from the store: its latest version for this caller, or
    /// `version`. Verified as a file is, then checked against the store's
    /// record, then installed as any package (the same consent window).
    pub(in crate::plugins) async fn install_from_store(&self, id: &str, version: Option<&str>, reinstalling: bool) -> Result<RpcReply, RpcError> {
        self.install_store_version(id, version, reinstalling, false).await
    }

    /// The verified version a sheet offers instead of the unverified one it
    /// asks about (`InstallVerifiedPlugin {id, hash}`): the version from
    /// this engine's own state for that item as shown (`hash`), an update's
    /// alternative or a first install's, never a caller's word; its record
    /// fetched now, and refused (`not_verified`) unless it says verified.
    pub(in crate::plugins) async fn install_verified_alternative(&self, id: &str, hash: &str) -> Result<RpcReply, RpcError> {
        self.rescan().await;
        let entry = self.entry(id).ok_or_else(|| failed(format!("There's no plugin {id}")))?;
        if !Self::by_person() {
            return Err(Self::needs_person(&entry.item.name));
        }
        if !entry.item.hash.eq_ignore_ascii_case(hash.trim()) {
            return Err(failed("It changed since it was shown: look again"));
        }
        let from_update = install::found(&self.inner.plugins_dir, id, &entry.item.hash).and_then(|f| f.verified_alternative).map(|a| a.version);
        let from_install = entry.store.as_ref().filter(|s| !s.verified).and_then(|s| s.verified_alternative.as_ref()).map(|a| a.version.clone());
        let Some(version) = from_update.or(from_install) else {
            return Err(failed(failure(install::reason::NOT_VERIFIED, format!("{} has no verified version to install instead", entry.item.name))));
        };
        self.install_store_version(id, Some(&version), false, true).await
    }

    /// [`Self::install_from_store`]; `verified_only`: refused unless the
    /// version's record, fetched now, says verified.
    async fn install_store_version(&self, id: &str, version: Option<&str>, reinstalling: bool, verified_only: bool) -> Result<RpcReply, RpcError> {
        let id = id.trim();
        manifest::check_id(id).map_err(failed)?;
        let person = Self::by_person();
        // (Before fetching anything: `install_package` refuses it too.)
        if !reinstalling && !person {
            self.rescan().await;
            if let Some(entry) = self.entry(id).filter(|e| !e.item.builtin && !e.item.own) {
                return Err(Self::needs_person(&entry.item.name));
            }
        }
        let (client, caller) = self.store_reader()?;
        let detail = store_detail(&client, &caller, id).await?;
        let named = version.map(str::trim).filter(|v| !v.is_empty());
        // What's installed: the version named; else, for a person, the
        // newest listed; for an agent, the newest verified (agents install
        // only what Zeron has verified).
        let wanted = match (named, person || reinstalling) {
            (Some(named), _) => named.to_owned(),
            (None, true) => detail.version.clone(),
            (None, false) => detail.verified_version.clone().ok_or_else(|| agent_unverified(id, &detail.version))?,
        };
        if wanted.is_empty() {
            return Err(failed(failure(client::reason::NOT_IN_STORE, format!("The store lists no version of {id}"))));
        }
        let (package, mut source) = self.fetch_version(&client, &caller, id, &wanted, reinstalling, &detail.author).await?;
        let verified = source.store().is_some_and(|s| s.verified);
        // The mark is the record's, fetched just now: an agent's install of
        // a version it doesn't carry is the person's to make.
        if !person && !reinstalling && !verified {
            return Err(agent_unverified(id, &wanted));
        }
        if verified_only && !verified {
            return Err(failed(failure(install::reason::NOT_VERIFIED, format!("The store's record of {id} {wanted} doesn't say it's verified, so it wasn't installed"))));
        }
        if verified_only && let Some(store) = source.store.as_mut() {
            store.instead_of_unverified = true;
        }
        // Not verified: the verified version its sheet can offer instead.
        if !verified
            && let Some(store) = source.store.as_mut()
        {
            store.verified_alternative = self.verified_alternative(&client, &caller, id, &wanted, &package.manifest.permissions, &detail.author).await;
        }
        let reply = self.install_package(package, source).await?;
        // Counted once per account (the edge's to say): a new install here.
        if let RpcReply::Value(value) = &reply
            && value["update"] == false
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let id = id.to_owned();
            runtime.spawn(async move {
                if let Err(error) = client.installed(&id, &caller).await {
                    tracing::debug!(plugin = %id, %error, "store: the install wasn't counted");
                }
            });
        }
        Ok(reply)
    }

    /// `id`'s `version` in the store (none: its newest listed), fetched,
    /// verified and checked against the store's record: the package and
    /// its source. `installed`: it's the version installed here (a
    /// reinstall), which may be yanked since.
    pub(in crate::plugins) async fn fetch_from_store(&self, id: &str, version: Option<&str>, installed: bool) -> Result<(install::Package, Source), RpcError> {
        manifest::check_id(id).map_err(failed)?;
        let (client, caller) = self.store_reader()?;
        let detail = store_detail(&client, &caller, id).await?;
        let wanted = version.map(str::trim).filter(|v| !v.is_empty()).unwrap_or(&detail.version).to_owned();
        if wanted.is_empty() {
            return Err(failed(failure(client::reason::NOT_IN_STORE, format!("The store lists no version of {id}"))));
        }
        self.fetch_version(&client, &caller, id, &wanted, installed, &detail.author).await
    }

    /// Version `version` of `id`, by its own record (`…/versions/:version`,
    /// fetched now: what it is, and whether it's verified), its package
    /// fetched and checked against that record.
    async fn fetch_version(
        &self,
        client: &StoreClient,
        caller: &Caller,
        id: &str,
        version: &str,
        installed: bool,
        publisher: &client::Publisher,
    ) -> Result<(install::Package, Source), RpcError> {
        let (_, found) = client.version(id, version, caller).await.map_err(failed)?;
        // Listed; a yanked one only to reinstall what's here.
        if !(found.status == "listed" && !found.yanked || installed && found.status == "yanked") {
            return Err(failed(failure(client::reason::NOT_IN_STORE, format!("Version {version} of {id} can't be installed any more"))));
        }
        let record = PackageRecord { hash: found.hash, sha256: found.sha256, bytes: found.bytes, key: found.key };
        let raw = client.package(id, version, caller, record.bytes).await.map_err(failed)?;
        let (id_owned, version_owned, publisher, verified) = (id.to_owned(), version.to_owned(), publisher.clone(), found.verified);
        let (package, mut store) = tokio::task::spawn_blocking(move || super::verify(raw, &id_owned, &version_owned, &record, &publisher, verified))
            .await
            .map_err(|_| failed("Couldn't read it"))?
            .map_err(failed)?;
        store.verified_at = found.verified.then_some(found.verified_at).flatten();
        let source = Source { kind: STORE.into(), url: None, path: None, git_ref: None, commit: None, version: String::new(), installed_at: 0, store: Some(store) };
        Ok((package, source))
    }

    /// The newest verified version of `id` this Zeron can run, other than
    /// `asked` (whose asks are `asks`), for a sheet's "instead": its record
    /// says verified, it's listed, and its package (fetched and checked as
    /// any install's) asks `added` and `removed` against `asked`. None when
    /// there's none, or the store can't say.
    async fn verified_alternative(&self, client: &StoreClient, caller: &Caller, id: &str, asked: &str, asks: &[String], publisher: &client::Publisher) -> Option<install::Alternative> {
        let zeron = version::zeron();
        let mut cursor: Option<String> = None;
        for _ in 0..ALTERNATIVE_PAGES {
            let page = client.versions(id, cursor.as_deref(), Some(50), caller).await.ok()?;
            let found = page["versions"].as_array().into_iter().flatten().find(|v| {
                v["verified"] == true
                    && v["status"] == "listed"
                    && v["yanked"] != true
                    && v["version"].as_str().is_some_and(|x| !same_version(x, asked))
                    && version::accepts(v["engines"]["zeron"].as_str(), &zeron).is_ok()
            });
            if let Some(found) = found {
                let version = found["version"].as_str()?.to_owned();
                let (package, source) = self.fetch_version(client, caller, id, &version, false, publisher).await.ok()?;
                // (Its own record, fetched with the package, says so too.)
                source.store().filter(|s| s.verified)?;
                let change = install::grant_change(asks, &package.manifest.permissions);
                return Some(install::Alternative {
                    version: plain(&version, 64),
                    added: change.added,
                    removed: change.removed,
                    notes: notes(found["notes"].as_str().unwrap_or_default(), listing::MAX_NOTES_BYTES),
                    verified_at: found["verified_at"].as_i64(),
                });
            }
            cursor = Some(page["next_cursor"].as_str()?.to_owned());
        }
        None
    }

    /// Whether the store's record says version `version` of `id` (content
    /// `hash`) is verified, asked now (an update about to be taken): `false`
    /// when the store can't be reached, so taking it asks you. A record that
    /// can't be read, names other content, is gone, or isn't installable
    /// (yanked, superseded, not listed) refuses the update: it never fails
    /// open.
    pub(in crate::plugins) async fn verified_now(&self, id: &str, version: &str, hash: &str) -> Result<bool, String> {
        let Ok((client, caller)) = self.store_reader() else { return Ok(false) };
        match client.version(id, version, &caller).await {
            Ok((_, record)) if record.status != "listed" || record.yanked => {
                Err(failure(client::reason::NOT_IN_STORE, format!("Version {version} of {id} can't be installed any more")))
            }
            Ok((_, record)) if record.hash.eq_ignore_ascii_case(hash) => Ok(record.verified),
            Ok(_) => Err(failure(client::reason::STORE_MISMATCH, format!("The store's record of {id} {version} names other content than the update found, so it wasn't taken"))),
            Err(error) if install::reason_of(&error) == Some(client::reason::STORE_UNREACHABLE) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// The batched check for every plugin installed from the store (or
    /// `only`): updates kept as any source's are (`install::Found`), blocked
    /// versions turned off, a block the store no longer names lifted.
    /// `(updates, errors)`, as `check_updates` says them.
    /// Every plugin installed from the store (or `only`), as a check asks
    /// about it.
    fn store_candidates(&self, only: Option<&str>) -> Vec<Candidate> {
        let state = lock(&self.inner.state).clone();
        lock(&self.inner.entries)
            .iter()
            .filter(|e| !e.item.builtin && !e.item.own && only.is_none_or(|id| id == e.item.id))
            .filter_map(|e| {
                let store = e.store.as_ref()?;
                let allowed = state.decisions.get(&e.item.id).filter(|d| d.allowed).map_or_else(|| e.record.clone(), |d| d.allowed_record());
                Some(Candidate {
                    id: e.item.id.clone(),
                    name: e.item.name.clone(),
                    version: e.item.version.clone(),
                    hash: e.item.hash.clone(),
                    publisher_id: store.publisher_id.clone(),
                    permissions: e.item.permissions.clone(),
                    allowed,
                    prerelease: state.prerelease.contains(&e.item.id),
                    key: e.key().map(str::to_owned),
                    verified: store.verified,
                    prefer: state.updates_for(&e.item.id),
                })
            })
            .collect()
    }

    /// The take-down feed after the last event applied here: each take-down
    /// of a version installed here turns it off, each restore lifts that,
    /// and either ends its mark (a restored version is unverified until
    /// verified again); a take-down of an update kept here drops it. With
    /// no place in the feed (a new device, one gone longer than the feed
    /// keeps, or a place past the feed's head), the full update check says
    /// what's blocked now and which marks have ended, and the feed carries
    /// on from its head. Only events after the place and at most the head
    /// count. Nothing about what's installed is sent.
    pub async fn check_takedowns(&self) {
        self.rescan().await;
        if self.store_candidates(None).is_empty() {
            return;
        }
        let Ok((client, caller)) = self.store_reader() else { return };
        let mut after = lock(&self.inner.state).takedowns_after;
        // (At most a few pages a check; the next one carries on.)
        for _ in 0..TAKEDOWN_PAGES {
            let feed = match client.takedowns(after).await {
                Ok(feed) => feed,
                Err(error) => {
                    tracing::debug!(%error, "store: the take-down feed couldn't be read");
                    return;
                }
            };
            if feed.reset || after.is_none_or(|a| a > feed.head) {
                let (_, errors) = self.check_store_updates(None).await;
                if errors.is_empty() {
                    self.end_marks(&client, &caller).await;
                    let _ = self.update_state(|state| state.takedowns_after = Some(feed.head));
                }
                return;
            }
            let events: Vec<_> = feed.events.iter().filter(|e| after.is_none_or(|a| e.seq > a) && e.seq <= feed.head).collect();
            let Some(last) = events.iter().map(|e| e.seq).max() else { return };
            let candidates = self.store_candidates(None);
            let mut down: BTreeMap<&str, (&Candidate, String)> = BTreeMap::new();
            let mut restored: Vec<&Candidate> = Vec::new();
            for event in events {
                // An update kept here that's taken down isn't taken.
                if event.kind == "take_down"
                    && let Some(c) = candidates.iter().find(|c| c.id == event.id)
                    && install::found(&self.inner.plugins_dir, &c.id, &c.hash).is_some_and(|f| same_version(&f.version, &event.version))
                {
                    install::forget_found(&self.inner.plugins_dir, &c.id);
                }
                let Some(c) = candidates.iter().find(|c| c.id == event.id && same_version(&event.version, &c.version)) else { continue };
                // Taken down or restored, in whatever order: no mark.
                self.unmark(c);
                match event.kind.as_str() {
                    "take_down" => {
                        restored.retain(|r| r.id != c.id);
                        down.insert(c.id.as_str(), (c, plain(&event.reason, 500)));
                    }
                    "restore" => {
                        down.remove(c.id.as_str());
                        restored.push(c);
                    }
                    _ => {}
                }
            }
            self.unblock(&restored);
            if !down.is_empty() {
                self.block(down.into_values().collect()).await;
            }
            after = Some(last);
            let _ = self.update_state(|state| state.takedowns_after = Some(last));
            if last >= feed.head {
                return;
            }
        }
    }

    /// Look over the account's publishes (`/me/plugins`) for any this
    /// device didn't make, and keep a notice of each for the app, with a
    /// desktop banner: once per version. Signed out, nothing. An account
    /// with no plugins is asked again only every few hours.
    pub async fn check_publishes(&self) {
        let Ok((client, caller)) = self.store_as_person().await else { return };
        let idle_for = publishes_idle(super::super::lifecycle::local_every());
        let idle = lock(&self.inner.publishes_checked).is_some_and(|(at, had)| !had && at.elapsed() < idle_for);
        if idle {
            return;
        }
        let mine = match client.my_plugins(&caller).await {
            Ok(mine) => mine,
            Err(error) => {
                tracing::debug!(%error, "store: the account's publishes couldn't be read");
                return;
            }
        };
        let had = mine["plugins"].as_array().is_some_and(|plugins| !plugins.is_empty());
        *lock(&self.inner.publishes_checked) = Some((std::time::Instant::now(), had));
        let account = own_publisher_id(&mine).unwrap_or_default();
        let own_key = signing::load_key(&self.inner.data_dir).map(|key| key.public()).unwrap_or_default();
        let before = lock(&self.inner.state).publishes.clone().filter(|seen| seen.account == account);
        let (mut seen, new) = notices::look(before.as_ref(), &mine, &own_key, |text| plain(text, 120));
        seen.account = account;
        if before.as_ref() == Some(&seen) && new.is_empty() {
            return;
        }
        let banners: Vec<String> = new.iter().map(notices::PublishNotice::words).collect();
        let _ = self.update_state(|state| {
            state.publishes = Some(seen);
            notices::add(&mut state.publish_notices, new);
        });
        if banners.is_empty() {
            return;
        }
        self.publish_list();
        if let Some(notify) = lock(&self.inner.notifier).clone() {
            for words in banners {
                notify("zeron", "Plugin published", &words, None);
            }
        }
    }

    pub(in crate::plugins) async fn check_store_updates(&self, only: Option<&str>) -> (Vec<Value>, Vec<Value>) {
        let candidates = self.store_candidates(only);
        if candidates.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let said = |c: &Candidate, error: &str| json!({ "id": c.id, "name": c.name, "error": error });
        let (client, caller) = match self.store_reader() {
            Ok(store) => store,
            Err(error) => return (Vec::new(), candidates.iter().map(|c| said(c, &error.to_string())).collect()),
        };
        let by_id: HashMap<&str, &Candidate> = candidates.iter().map(|c| (c.id.as_str(), c)).collect();
        let mut errors = Vec::new();
        let mut offers: BTreeMap<String, Offer> = BTreeMap::new();
        let mut blocks = Vec::new();
        // The plugins the store answered about (offline batches aren't).
        let mut answered: Vec<&Candidate> = Vec::new();
        // Those with an entry it sent that couldn't be read: whatever block
        // they have stays (it fails closed).
        let mut unread: BTreeSet<&str> = BTreeSet::new();
        // At most the edge's limit per request.
        for batch in candidates.chunks(MAX_UPDATE_PLUGINS) {
            let ask = UpdatesAsk {
                zeron: version::zeron().to_string(),
                plugins: batch
                    .iter()
                    .map(|c| Asked { id: c.id.clone(), version: c.version.clone(), hash: c.hash.clone(), prerelease: c.prerelease })
                    .collect(),
            };
            let reply = match client.updates(&ask, &caller).await {
                Ok(reply) => reply,
                // Offline: what's installed keeps running; the next check asks again.
                Err(error) => {
                    errors.extend(batch.iter().map(|c| said(c, &error)));
                    continue;
                }
            };
            answered.extend(batch.iter());
            for unreadable in &reply.unreadable {
                match unreadable.as_deref().and_then(|id| by_id.get(id)) {
                    Some(c) => {
                        unread.insert(c.id.as_str());
                        errors.push(said(c, &failure(client::reason::STORE_ERROR, "The store's answer about it couldn't be read")));
                    }
                    // (Whose, it doesn't say: none of this batch's blocks lifts.)
                    None => unread.extend(batch.iter().map(|c| c.id.as_str())),
                }
            }
            blocks.extend(reply.blocked);
            // One offer per plugin asked about; anything else the store says
            // is none of this device's business.
            for offer in reply.updates {
                if by_id.contains_key(offer.id.as_str()) {
                    offers.entry(offer.id.clone()).or_insert(offer);
                }
            }
        }
        // One block per plugin, for the version installed (the first the
        // store says: a hostile answer repeating one costs nothing more).
        let mut blocked: BTreeMap<&str, (&Candidate, String)> = BTreeMap::new();
        for block in &blocks {
            if let Some(c) = by_id.get(block.id.as_str())
                && same_version(&block.version, &c.version)
            {
                blocked.entry(c.id.as_str()).or_insert_with(|| (*c, plain(&block.reason, 500)));
            }
        }
        // A block here the store no longer names (restored): lifted.
        let lifted: Vec<&Candidate> = answered.into_iter().filter(|c| !blocked.contains_key(c.id.as_str()) && !unread.contains(c.id.as_str())).collect();
        self.unblock(&lifted);
        if !blocked.is_empty() {
            self.block(blocked.into_values().collect()).await;
        }
        let plugins_dir = self.inner.plugins_dir.clone();
        let mut updates = Vec::new();
        for c in &candidates {
            let Some(offer) = offers.remove(&c.id) else {
                install::forget_found(&plugins_dir, &c.id);
                continue;
            };
            // Which to stage: the newest (marked when it isn't verified, the
            // newest verified beside it), or, by this device's choice, the
            // newest verified.
            let (staged, beside) = match c.prefer {
                UpdatePreference::Newest => (Some(&offer), offer.verified_update.as_deref()),
                UpdatePreference::Verified if offer.verified => (Some(&offer), None),
                UpdatePreference::Verified => (offer.verified_update.as_deref(), None),
            };
            let Some(staged) = staged else {
                // Only unverified updates, and this device stages verified
                // ones: said, not kept.
                install::forget_found(&plugins_dir, &c.id);
                updates.push(json!({
                    "id": c.id, "name": c.name, "version": offer.version, "verified": false, "held": true,
                    "installed_verified": c.verified,
                }));
                continue;
            };
            match self.store_found(&client, &caller, c, staged, beside).await {
                Ok(found) => {
                    let mut shown = found.shown();
                    shown["id"] = json!(c.id);
                    shown["name"] = json!(c.name);
                    updates.push(shown);
                }
                Err(error) => errors.push(said(c, &error)),
            }
        }
        self.catch_up_verified(&client, &caller, &candidates).await;
        (updates, errors)
    }

    /// Each version installed here unverified whose own record, read now,
    /// says verified and names the same content: its install record says so
    /// from now on (`verified`, `verified_at`), and the list shows it. Only
    /// ever that way: a record that can't be read, names other content, or
    /// says not verified changes nothing (a take-down is the feed's).
    async fn catch_up_verified(&self, client: &StoreClient, caller: &Caller, candidates: &[Candidate]) {
        let mut caught = false;
        for c in candidates.iter().filter(|c| !c.verified) {
            let Ok((_, record)) = client.version(&c.id, &c.version, caller).await else { continue };
            if !record.verified || !record.hash.eq_ignore_ascii_case(&c.hash) {
                continue;
            }
            let Some(entry) = self.entry(&c.id).filter(|e| e.item.hash.eq_ignore_ascii_case(&c.hash)) else { continue };
            let dir = Path::new(&entry.item.dir);
            if let Some(mut source) = Source::load(dir)
                && let Some(store) = source.store.as_mut().filter(|s| !s.verified && s.hash.eq_ignore_ascii_case(&c.hash))
            {
                store.verified = true;
                store.verified_at = record.verified_at;
                store.verified_alternative = None;
                if source.save(dir).is_ok() {
                    tracing::info!(plugin = %c.id, version = %c.version, "store: the version installed is verified now");
                    caught = true;
                }
            }
        }
        if caught {
            self.kick(super::super::Kick::Rescan);
        }
    }

    /// `offer` for `c`, fetched and verified (or the copy already kept, when
    /// it's the same version), compared with what's installed, and kept;
    /// `beside`, the newest verified version when `offer` isn't, named with
    /// it.
    async fn store_found(&self, client: &StoreClient, caller: &Caller, c: &Candidate, offer: &Offer, beside: Option<&Offer>) -> Result<Found, String> {
        let plugins_dir = self.inner.plugins_dir.clone();
        let record = PackageRecord { hash: offer.hash.clone(), sha256: offer.sha256.clone(), bytes: offer.bytes, key: offer.key.clone() };
        let kept_path = install::found_package_path(&plugins_dir, &c.id);
        let kept = install::found(&plugins_dir, &c.id, &c.hash)
            .filter(|f| f.refused.is_none() && f.hash == offer.hash && f.version == offer.version && kept_path.is_file());
        let (id, version, publisher) = (c.id.clone(), offer.version.clone(), offer.publisher());
        // (`verified` here is the offer's, for the update's sheet; taking
        // it asks the store's record again: `Plugins::update`.)
        let (package, store) = if kept.is_some() {
            // Verified when it was found; read back as any kept update is.
            let package = tokio::task::spawn_blocking(move || package::read(&kept_path, &install::WORDS).and_then(install::parse))
                .await
                .map_err(|_| "Couldn't read it".to_string())??;
            let (publisher, publisher_id) = super::vouched(&publisher)?;
            if package.hash != offer.hash || offer.key.as_deref().is_some_and(|k| k != package.key) {
                return Err(failure(client::reason::STORE_MISMATCH, "The update kept isn't the store's"));
            }
            let store = install::StoreSource { id: c.id.clone(), publisher, publisher_id, hash: package.hash.clone(), key: Some(package.key.clone()), verified: offer.verified, verified_at: None, verified_alternative: None, instead_of_unverified: false, replaced: None };
            (package, store)
        } else {
            let raw = client.package(&c.id, &offer.version, caller, record.bytes).await?;
            let verified = offer.verified;
            tokio::task::spawn_blocking(move || super::verify(raw, &id, &version, &record, &publisher, verified))
                .await
                .map_err(|_| "Couldn't read it".to_string())??
        };
        let refused = if !later(&package.manifest.version, &c.version) {
            Some(install::not_newer(&package.manifest.version, &c.version))
        } else {
            install::compatible(&package.manifest).err()
        };
        let change = install::grant_change(&c.permissions, &package.manifest.permissions);
        let diff = capability::diff(&c.allowed, &capability::record(&package.manifest));
        // (By the owner's id: a username moved to another account is
        // another owner.)
        let new_owner = store.publisher_id != c.publisher_id;
        let found = Found {
            from: c.hash.clone(),
            version: package.manifest.version.clone(),
            hash: package.hash.clone(),
            added: change.added,
            removed: change.removed,
            refused,
            source: Some(Source { kind: STORE.into(), url: None, path: None, git_ref: None, commit: None, version: String::new(), installed_at: 0, store: Some(store) }),
            digest_diff: diff.grew().then(|| diff.to_value()),
            // (The store's record decides whose it is: shown, so the update
            // can say its signing key changed.)
            new_key: c.key.as_deref().is_some_and(|key| key != package.key),
            new_owner,
            notes: notes(&offer.notes, listing::MAX_NOTES_BYTES),
            store_verified: Some(offer.verified),
            store_verified_at: offer.verified.then_some(offer.verified_at).flatten(),
            installed_verified: Some(c.verified),
            verified_alternative: beside.filter(|b| b.verified).map(|b| install::Alternative {
                version: plain(&b.version, 64),
                added: b.added.iter().map(|p| plain(p, 64)).collect(),
                removed: b.removed.iter().map(|p| plain(p, 64)).collect(),
                notes: notes(&b.notes, listing::MAX_NOTES_BYTES),
                verified_at: b.verified_at,
            }),
        };
        let keep = found.refused.is_none().then_some(&package);
        install::keep_found(&plugins_dir, &c.id, &found, keep)?;
        Ok(found)
    }

    /// `c`'s installed version is blocked by the store: turned off, the
    /// reason kept, and you're told. Not again once you turned it back on.
    /// Turn off each blocked version (with the store's reason), after one
    /// rescan for all of them.
    async fn block(&self, blocked: Vec<(&Candidate, String)>) {
        self.rescan().await;
        for (c, reason) in blocked {
            self.block_one(c, &reason);
        }
    }

    /// The store no longer blocks these installed versions (restored): the
    /// block goes, and so does the mark (a restored version is unverified),
    /// so one turned off by the block asks you again.
    fn unblock(&self, lifted: &[&Candidate]) {
        for c in lifted {
            let mut lifted_here = false;
            let saved = self.update_state(|state| {
                if !state.blocked.get(&c.id).is_some_and(|b| same_version(&b.version, &c.version)) {
                    return;
                }
                let overridden = state.blocked.remove(&c.id).is_some_and(|b| b.overridden);
                lifted_here = true;
                if let Some(d) = state.decisions.get_mut(&c.id)
                    && !overridden
                    && d.allowed
                {
                    d.off = false;
                    d.reask = true;
                }
            });
            if saved.is_err() || !lifted_here {
                continue;
            }
            if let Some(entry) = self.entry(&c.id) {
                let dir = Path::new(&entry.item.dir);
                if let Some(mut source) = Source::load(dir)
                    && let Some(store) = source.store.as_mut()
                {
                    store.verified = false;
                    store.verified_at = None;
                    let _ = source.save(dir);
                }
            }
            tracing::info!(plugin = %c.id, version = %c.version, "store: a take-down was lifted");
            self.kick(super::super::Kick::Rescan);
        }
    }

    /// `c`'s installed version is no longer verified here: its install
    /// record says so (`verified`, `verified_at`), and the list shows it.
    fn unmark(&self, c: &Candidate) {
        let Some(entry) = self.entry(&c.id).filter(|e| e.item.version == c.version && e.item.hash.eq_ignore_ascii_case(&c.hash)) else { return };
        let dir = Path::new(&entry.item.dir);
        if let Some(mut source) = Source::load(dir)
            && let Some(store) = source.store.as_mut().filter(|s| s.verified)
        {
            store.verified = false;
            store.verified_at = None;
            if source.save(dir).is_ok() {
                tracing::info!(plugin = %c.id, version = %c.version, "store: the version installed is no longer verified");
                self.kick(super::super::Kick::Rescan);
            }
        }
    }

    /// After the feed starts over (its events missed): each version
    /// installed here marked verified whose own record, read now, says it
    /// isn't (taken down and restored meanwhile) loses its mark.
    async fn end_marks(&self, client: &StoreClient, caller: &Caller) {
        for c in self.store_candidates(None).iter().filter(|c| c.verified) {
            if let Ok((_, record)) = client.version(&c.id, &c.version, caller).await
                && !record.verified
            {
                self.unmark(c);
            }
        }
    }

    fn block_one(&self, c: &Candidate, reason: &str) {
        // Still the version the store blocked (not one installed while the
        // check was out).
        if self.entry(&c.id).is_none_or(|e| e.item.version != c.version || e.item.hash != c.hash) {
            return;
        }
        // A blocked version shows no mark (turned back on, it still has none).
        self.unmark(c);
        let mut newly = false;
        let saved = self.update_state(|state| {
            if state.blocked.get(&c.id).is_some_and(|b| same_version(&b.version, &c.version)) {
                return;
            }
            newly = true;
            state.blocked.insert(c.id.clone(), Blocked { version: c.version.clone(), reason: reason.to_owned(), at: now_secs(), overridden: false });
            if let Some(d) = state.decisions.get_mut(&c.id) {
                d.off = true;
            }
        });
        if let Err(error) = saved {
            tracing::warn!(plugin = %c.id, %error, "store: couldn't keep a block");
            return;
        }
        if newly {
            tracing::warn!(plugin = %c.id, version = %c.version, "store: a blocked version was turned off");
            let why = if reason.is_empty() { String::new() } else { format!(": {reason}") };
            self.key_notice(
                &format!("blocked:{}:{}", c.id, c.version),
                "plugin_blocked",
                None,
                "Plugin turned off",
                &format!(
                    "{} {} was taken down from the store and turned off{why}. You can turn it back on in Settings → Plugins.",
                    c.name, c.version
                ),
            );
            self.kick(super::super::Kick::Rescan);
        }
    }
}

/// `{ok: false, error: "not_published"}`: a deploy key is for a plugin you
/// have in the store already.
/// The store's page for `id`: its newest listed version, its newest
/// verified one, who owns it.
async fn store_detail(client: &StoreClient, caller: &Caller, id: &str) -> Result<client::Detail, RpcError> {
    client.plugin(id, caller).await.map(|(_, detail)| detail).map_err(failed)
}

/// An agent's install of a version Zeron hasn't verified: the person's to
/// make, from the plugin's page.
fn agent_unverified(id: &str, version: &str) -> RpcError {
    failed(failure(
        install::reason::NEEDS_PERSON,
        format!(
            "{id}{} isn't verified by Zeron: an agent installs only versions Zeron has verified; the person installs the rest from its page",
            if version.is_empty() { String::new() } else { format!(" {version}") }
        ),
    ))
}

/// `{ok: false, error: "not_owner", ids}`: a key's scope names plugins
/// that aren't the account's in the store.
fn not_owner(ids: &[&str]) -> Value {
    let mut value = not_ok(&failure(
        "not_owner",
        format!("{} isn't yours in the store: a key publishes plugins you already have there (a first version comes from a signed-in device)", ids.join(", ")),
    ));
    value["ids"] = json!(ids);
    value
}

/// The account's publisher id, as `/me/plugins` gives it at its top,
/// plugins or none.
fn own_publisher_id(mine: &Value) -> Option<String> {
    mine["publisher_id"]
        .as_str()
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
}

/// The bytes under `dir`.
fn size_of(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => size_of(&e.path()),
            _ => e.metadata().map(|m| m.len()).unwrap_or(0),
        })
        .sum()
}

/// Keep the files of the newest few versions of each plugin cached, and
/// all of them under [`MAX_CACHE_BYTES`] (the oldest go first).
fn prune_cache(plugin_cache: &Path) {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
    let dirs = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect()
    };
    let mut versions = dirs(plugin_cache);
    versions.sort_by_key(|p| std::cmp::Reverse(modified(p)));
    for old in versions.into_iter().skip(CACHED_VERSIONS) {
        let _ = std::fs::remove_dir_all(old);
    }
    let Some(root) = plugin_cache.parent() else { return };
    let mut all: Vec<(std::time::SystemTime, PathBuf, u64)> =
        dirs(root).iter().flat_map(|plugin| dirs(plugin)).map(|v| (modified(&v), v.clone(), size_of(&v))).collect();
    let mut total: u64 = all.iter().map(|(_, _, n)| n).sum();
    all.sort_by_key(|(at, _, _)| *at);
    for (_, old, bytes) in all {
        if total <= MAX_CACHE_BYTES {
            break;
        }
        let _ = std::fs::remove_dir_all(&old);
        total = total.saturating_sub(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_strictly_when_they_can() {
        assert!(later("1.0.0", "1.0.0-rc.1") && later("1.10.0", "1.9.0"));
        assert!(!later("1.0.0+b", "1.0.0+a"), "build metadata isn't a later version");
        assert!(same_version("1.0.0+a", "1.0.0") && !same_version("1.0.0", "1.0.1"));
        assert!(later("2", "1.9"), "a version that isn't strict compares as sideloading does");
    }

    #[test]
    fn store_words_are_kept_plain() {
        assert_eq!(plain("bad\u{7}thing\nhere ", 100), "bad thing here");
        assert_eq!(plain(&"x".repeat(900), 500).len(), 500);
        assert_eq!(not_ok("id_taken: Someone has it"), json!({ "ok": false, "error": "id_taken", "message": "Someone has it" }));
        assert_eq!(not_ok("plain words"), json!({ "ok": false, "error": "failed", "message": "plain words" }));
        // A listing problem's fields beside its sentence, those there are.
        let fields = json!({ "field": "name", "cause": "mark_word", "word": "Verified", "other": "x" });
        assert_eq!(refused("bad_listing: Its name says \"verified\"", &fields), json!({ "ok": false, "error": "bad_listing", "message": "Its name says \"verified\"", "field": "name", "cause": "mark_word", "word": "Verified" }));
        assert_eq!(refused("too_large: big", &json!({ "field": null })), not_ok("too_large: big"));
        assert_eq!(media_type("shots/A.PNG"), "image/png");
    }
}
