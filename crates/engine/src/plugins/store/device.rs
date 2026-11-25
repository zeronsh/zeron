//! The store's IPC methods, and installing and updating plugins from it:
//! the IO around `store::verify`, `store::publish` and the client.
//!
//! Who may do what (`zeron_rpc::local_only`, `agent_allowed`): every store
//! method is this device's own (never a peer's through the relay). Agents
//! may search and read the store, install from it (it lands waiting for the
//! person, as any install does) and check one of the person's plugins
//! against its rules; submitting a plugin, acting on its versions and its
//! deploy tokens are the person's, in Zeron's window or the CLI. Submitting
//! also needs the ticket `PreparePluginPublish` gives this device's app or
//! CLI for the plugin as checked (`reviews::Kind::Publish`), so what's sent
//! is what the person saw checked.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use zeron_rpc::{RpcError, RpcReply, methods, parse_params};

use super::client::{self, Caller, Installed as Asked, Kind, Offer, PackageRecord, Search, StoreClient, UpdatesAsk};
use super::deploy_token::Scope;
use super::notices;
use super::publish;
use crate::plugins::install::{self, Found, STORE, Source, failure};
use crate::plugins::listing::{self, CATEGORIES};
use crate::plugins::status::{Blocked, State, UpdatePreference};
use crate::plugins::version::{self, Version};
use crate::plugins::{Plugins, capability, failed, lock, manifest, now_secs, package, reviews, signing, template_blocks, template_packages};

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
/// The most items (plugins and templates together) one update check asks
/// about (the edge's limit, `max_update_items`).
pub const MAX_UPDATE_ITEMS: usize = 200;
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
            | methods::SET_TEMPLATE_UPDATES
            | methods::SET_TEMPLATE_PRERELEASES
            | methods::PREPARE_PLUGIN_PUBLISH
            | methods::PUBLISH_PLUGIN
            | methods::PREPARE_TEMPLATE_PUBLISH
            | methods::PUBLISH_TEMPLATE
            | methods::STORE_MY_PLUGINS
            | methods::STORE_VERSION_ACTION
            | methods::LIST_DEPLOY_TOKENS
            | methods::CREATE_DEPLOY_TOKEN
            | methods::REVOKE_DEPLOY_TOKEN
            | methods::STORE_REPORT
            | methods::ACK_STORE_NOTICES
    )
}

/// The kind of item a call is about: a plugin, unless it says. One this
/// Zeron doesn't know is refused, so it's never taken as the other.
fn kind_of(params: &Value) -> Result<Kind, RpcError> {
    match &params["kind"] {
        Value::Null => Ok(Kind::Plugin),
        Value::String(kind) if kind == "plugin" => Ok(Kind::Plugin),
        Value::String(kind) if kind == "template" => Ok(Kind::Template),
        _ => Err(failed("The kind is plugin or template")),
    }
}

/// `id` can be an item of `kind`'s in the store.
fn check_item_id(kind: Kind, id: &str) -> Result<(), RpcError> {
    match kind {
        Kind::Plugin => manifest::check_id(id).map_err(failed),
        Kind::Template if crate::plugins::templates::valid_id(id) => Ok(()),
        Kind::Template => Err(failed(format!("\"{}\" can't be a template's id", plain(id, 64)))),
    }
}

/// A template's answer says what it is, at its top: one from a Zeron that
/// reads no kind doesn't, so it's never taken for one.
fn says_kind(reply: &mut Value, kind: Kind) {
    if kind == Kind::Template {
        reply["kind"] = json!("template");
    }
}

/// A refusal of a template's.
fn not_template(error: &str) -> Value {
    let mut value = not_ok(error);
    says_kind(&mut value, Kind::Template);
    value
}

/// What a store template's consent window draws of its record.
#[derive(Deserialize)]
pub(in crate::plugins) struct ShownRecord {
    verified: bool,
    publisher: String,
    publisher_id: String,
    version: String,
}

/// What's installed here of one kind ([`Plugins::installed_of`]): templates'
/// rows by the store's id; plugins' are the list's.
enum InstalledOf {
    Plugins,
    Templates(BTreeMap<String, Value>),
}

impl InstalledOf {
    /// What the store's pages say of `id` installed here.
    fn of(&self, plugins: &Plugins, id: &str) -> Option<Value> {
        let row = match self {
            InstalledOf::Plugins => return plugins.installed_here(id),
            InstalledOf::Templates(rows) => rows.get(id)?,
        };
        let source = &row["imported"]["source"];
        let mut installed = json!({
            "version": row["imported"]["version"],
            "hash": row["imported"]["hash"],
            "status": row["status"],
            "store": true,
            "folder": row["id"],
            "verified": source["verified"],
            "edited": row["edited"] == true,
            "updates": lock(&plugins.inner.state).template_updates(id).word(),
        });
        for key in ["update", "blocked"] {
            if let Some(value) = row.get(key) {
                installed[key] = value.clone();
            }
        }
        Some(installed)
    }
}

/// A record's mark and publisher, as a review is good for them.
pub(in crate::plugins) fn record_state(record: &install::StoreSource) -> String {
    json!([record.verified, record.publisher, record.publisher_id]).to_string()
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
struct CreateTokenParams {
    name: String,
    /// `{all: true, all_kinds?}` or `{plugins: [id…]}`.
    scope: ScopeParams,
    /// Epoch ms.
    #[serde(default)]
    expires: Option<i64>,
}

#[derive(Deserialize)]
struct ScopeParams {
    #[serde(default)]
    all: bool,
    /// Beside `all` alone: `["plugin"]` (none: that) or `["plugin",
    /// "template"]`.
    #[serde(default)]
    all_kinds: Option<Vec<String>>,
    /// Ids of either kind.
    #[serde(default)]
    plugins: Option<Vec<String>>,
}

impl ScopeParams {
    fn scope(&self) -> Result<Scope, String> {
        match (self.all, &self.all_kinds, &self.plugins) {
            (true, kinds, None) => Scope::all(kinds.as_deref()),
            (false, None, Some(ids)) => Scope::plugins(ids),
            _ => Err("A token's scope is all of your plugins ({all: true}) or the ones it names ({plugins: [...]})".into()),
        }
    }
}

#[derive(Deserialize)]
struct AckNoticesParams {
    ids: Vec<String>,
}

#[derive(Deserialize)]
struct RevokeTokenParams {
    id: String,
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
    /// The key that signed the version installed; an unsigned one's (a
    /// deploy token's), the newest signed version's before it.
    key: Option<String>,
    /// The version installed was verified (its record's, at install).
    verified: bool,
    /// Which update this device stages for it.
    prefer: UpdatePreference,
}

/// What's said of a check whose plugin or template had other updates chosen
/// for it meanwhile.
const CHOSEN_ANEW: &str = "You chose other updates of it while Zeron looked; check again";

/// What a full check of the store found.
struct StoreCheck {
    updates: Vec<Value>,
    errors: Vec<Value>,
    /// Every template here was answered about, and what's blocked kept.
    templates_settled: bool,
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
                let kind = kind_of(&params)?;
                let search: Search = parse_params(params)?;
                let (client, caller) = self.store_reader()?;
                let mut reply = client.of(kind).search(&search, &caller).await.map_err(failed)?;
                let here = self.installed_of(kind).await;
                if let Some(items) = reply.get_mut("items").and_then(Value::as_array_mut) {
                    for item in items {
                        clean_card(item);
                        if let Some(installed) = item["id"].as_str().and_then(|id| here.of(self, id)) {
                            item["installed"] = installed;
                        }
                        says_kind(item, kind);
                    }
                }
                says_kind(&mut reply, kind);
                RpcReply::value(&reply)
            }
            methods::STORE_PLUGIN => {
                let kind = kind_of(&params)?;
                let p: PluginParams = parse_params(params)?;
                check_item_id(kind, &p.id)?;
                let (client, caller) = self.store_reader()?;
                let (mut reply, _) = client.of(kind).plugin(&p.id, &caller).await.map_err(failed)?;
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
                if let Some(installed) = self.installed_of(kind).await.of(self, &p.id) {
                    reply["installed"] = installed;
                }
                says_kind(&mut reply, kind);
                RpcReply::value(&reply)
            }
            methods::STORE_VERSIONS => {
                let kind = kind_of(&params)?;
                let p: VersionsParams = parse_params(params)?;
                check_item_id(kind, &p.id)?;
                let (client, caller) = self.store_reader()?;
                let mut reply = client.of(kind).versions(&p.id, p.cursor.as_deref(), p.limit, &caller).await.map_err(failed)?;
                clean_card(&mut reply);
                says_kind(&mut reply, kind);
                RpcReply::value(&reply)
            }
            methods::STORE_VERSION => {
                let kind = kind_of(&params)?;
                let p: VersionParams = parse_params(params)?;
                check_item_id(kind, &p.id)?;
                let (client, caller) = self.store_reader()?;
                let (mut reply, _) = client.of(kind).version(&p.id, p.version.trim(), &caller).await.map_err(failed)?;
                says_kind(&mut reply, kind);
                if let Some(text) = reply.get_mut("notes").and_then(|n| n.as_str().map(str::to_owned)) {
                    reply["notes"] = json!(notes(&text, listing::MAX_NOTES_BYTES));
                }
                RpcReply::value(&reply)
            }
            methods::STORE_STATS => {
                let kind = kind_of(&params)?;
                let (client, _) = self.store_reader()?;
                let mut reply = client.of(kind).stats().await.map_err(failed)?;
                says_kind(&mut reply, kind);
                RpcReply::value(&reply)
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
                self.choose_updates(|state| match (&p.id, prefer) {
                    (Some(id), Some(prefer)) => {
                        state.store_updates_for.insert(id.clone(), prefer);
                    }
                    (Some(id), None) => {
                        state.store_updates_for.remove(id);
                    }
                    (None, prefer) => state.store_updates = prefer,
                })
                .await?;
                let state = lock(&self.inner.state).clone();
                let device = state.device_updates();
                let effective = p.id.as_deref().map_or(device, |id| state.updates_for(id));
                RpcReply::value(&json!({ "prefer": effective.word(), "device": device.word() }))
            }
            methods::STORE_CATEGORIES => {
                // (Templates have none.)
                if kind_of(&params)? == Kind::Template {
                    return RpcReply::value(&json!({ "kind": "template", "categories": [] }));
                }
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
                let kind = kind_of(&params)?;
                let p: FileParams = parse_params(params)?;
                check_item_id(kind, &p.id)?;
                let mut reply = self.store_file(kind, p).await.map_err(failed)?;
                says_kind(&mut reply, kind);
                RpcReply::value(&reply)
            }
            methods::SET_TEMPLATE_UPDATES => {
                let p: UpdatesParams = parse_params(params)?;
                let id = p.id.as_deref().map(str::trim).unwrap_or_default().to_owned();
                let prefer = match p.prefer.as_deref().map(str::trim).filter(|w| !w.is_empty()) {
                    None => None,
                    Some(word) => Some(UpdatePreference::parse(word).ok_or_else(|| failed("Updates are newest or verified"))?),
                };
                self.store_template_here(&id).await?;
                self.choose_updates(|state| match prefer {
                    Some(prefer) => {
                        state.template_updates_for.insert(id.clone(), prefer);
                    }
                    None => {
                        state.template_updates_for.remove(&id);
                    }
                })
                .await?;
                let state = lock(&self.inner.state).clone();
                RpcReply::value(&json!({ "prefer": state.template_updates(&id).word(), "device": state.device_updates().word() }))
            }
            methods::SET_TEMPLATE_PRERELEASES => {
                let p: PrereleaseParams = parse_params(params)?;
                let id = p.id.trim().to_owned();
                self.store_template_here(&id).await?;
                self.choose_updates(|state| {
                    if p.on {
                        state.template_prerelease.insert(id.clone());
                    } else {
                        state.template_prerelease.remove(&id);
                    }
                })
                .await?;
                RpcReply::value(&json!({ "id": id, "on": p.on }))
            }
            methods::SET_PLUGIN_PRERELEASES => {
                let p: PrereleaseParams = parse_params(params)?;
                self.rescan().await;
                let entry = self.entry(&p.id).ok_or_else(|| failed(format!("There's no plugin {}", p.id)))?;
                if entry.store.is_none() {
                    return Err(failed("Only a plugin installed from the store has pre-releases to take"));
                }
                self.choose_updates(|state| {
                    if p.on {
                        state.prerelease.insert(p.id.clone());
                    } else {
                        state.prerelease.remove(&p.id);
                    }
                })
                .await?;
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
            methods::PREPARE_TEMPLATE_PUBLISH => {
                let p: IdParams = parse_params(params)?;
                let id = p.id.trim().to_owned();
                let mut prepared = self.own_template(&id).await.map_err(failed)?;
                // As a plugin's: newer than what the store has, as it said a
                // moment ago, when it can be asked.
                let versions = match self.store_as_person().await {
                    Ok((client, caller)) => client.of(Kind::Template).my_plugins(&caller).await.ok().map(|mine| {
                        mine["templates"].as_array().into_iter().flatten().find(|item| item["id"] == prepared.id()).map(|item| item["versions"].clone())
                    }),
                    Err(_) => None,
                };
                if let Some(Some(Value::Array(versions))) = &versions
                    && let Some(problem) = publish::newer_problem(prepared.version(), versions)
                {
                    prepared.problems.push(problem);
                }
                let mut reply = prepared.shown();
                reply["store_checked"] = json!(versions.is_some());
                if versions.is_none() {
                    reply["note"] = json!("Couldn't ask the store which versions it has: the store has the last word on whether this one is newer");
                }
                if prepared.ok() && zeron_rpc::caller().is_local() {
                    let ticket = lock(&self.inner.reviews).mint(reviews::Kind::PublishTemplate, reviews::Origin::Local, &id, &prepared.hash)?;
                    reply["ticket"] = json!(ticket);
                }
                RpcReply::value(&reply)
            }
            methods::PUBLISH_TEMPLATE => {
                let p: PublishParams = parse_params(params)?;
                if !zeron_rpc::caller().is_local() {
                    return Err(reviews::needed("Publishing a template"));
                }
                let id = p.id.trim().to_owned();
                lock(&self.inner.reviews).redeem(reviews::Kind::PublishTemplate, &reviews::Origin::Local, &id, &p.hash, p.ticket.as_deref())?;
                RpcReply::value(&self.publish_template(&id, &p.hash).await)
            }
            methods::STORE_MY_PLUGINS => {
                let kind = kind_of(&params)?;
                let (client, caller) = self.store_as_person().await.map_err(failed)?;
                let mut reply = client.of(kind).my_plugins(&caller).await.map_err(failed)?;
                says_kind(&mut reply, kind);
                for plugin in reply.get_mut(kind.segment()).and_then(Value::as_array_mut).into_iter().flatten() {
                    clean_card(plugin);
                }
                RpcReply::value(&reply)
            }
            methods::STORE_VERSION_ACTION => {
                let kind = kind_of(&params)?;
                let p: VersionActionParams = parse_params(params)?;
                check_item_id(kind, &p.id)?;
                Version::parse(&p.version).map_err(failed)?;
                // (What an author does to a version: amendment 45.)
                if !matches!(p.action.as_str(), "yank" | "unyank") {
                    return Err(failed("The action is yank or unyank"));
                }
                let (client, caller) = self.store_as_person().await.map_err(failed)?;
                let reply = client.of(kind).version_action(&p.id, &p.version, &p.action, &caller).await;
                RpcReply::value(&match reply {
                    Ok(mut value) => {
                        value["ok"] = json!(true);
                        says_kind(&mut value, kind);
                        value
                    }
                    Err(error) => {
                        let mut value = not_ok(&error);
                        says_kind(&mut value, kind);
                        value
                    }
                })
            }
            methods::LIST_DEPLOY_TOKENS => {
                let (client, caller) = self.store_as_person().await.map_err(failed)?;
                RpcReply::value(&client.tokens(&caller).await.map_err(failed)?)
            }
            methods::CREATE_DEPLOY_TOKEN => {
                let p: CreateTokenParams = parse_params(params)?;
                RpcReply::value(&self.create_deploy_token(p).await)
            }
            methods::STORE_REPORT => {
                let kind = kind_of(&params)?;
                let p: ReportParams = parse_params(params)?;
                check_item_id(kind, &p.id)?;
                if !REPORT_REASONS.contains(&p.reason.as_str()) {
                    return Err(failed(format!("The reason is one of {}", REPORT_REASONS.join(", "))));
                }
                let text = p.text.as_deref().map(|t| notes(t, 2000)).filter(|t| !t.is_empty());
                let (client, caller) = self.store_as_person().await.map_err(failed)?;
                RpcReply::value(&match client.of(kind).report(&p.id, &p.reason, text.as_deref(), &caller).await {
                    Ok(mut value) => {
                        value["ok"] = json!(true);
                        says_kind(&mut value, kind);
                        value
                    }
                    Err(error) => {
                        let mut value = not_ok(&error);
                        says_kind(&mut value, kind);
                        value
                    }
                })
            }
            methods::ACK_STORE_NOTICES => {
                let p: AckNoticesParams = parse_params(params)?;
                self.update_state(|state| state.publish_notices.retain(|n| !p.ids.contains(&n.id)))?;
                self.publish_list();
                RpcReply::value(&json!({}))
            }
            methods::REVOKE_DEPLOY_TOKEN => {
                let p: RevokeTokenParams = parse_params(params)?;
                let id = p.id.trim();
                if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
                    return Err(failed("That isn't a deploy token's id: give it as `zeron plugin token list` shows it"));
                }
                let (client, caller) = self.store_as_person().await.map_err(failed)?;
                client.revoke_token(id, &caller).await.map_err(failed)?;
                RpcReply::value(&json!({ "revoked": id }))
            }
            _ => Err(RpcError::UnknownMethod(method.to_owned())),
        }
    }

    /// Refused unless store template `id` (the store's) is installed here,
    /// in whatever folder.
    async fn store_template_here(&self, id: &str) -> Result<(), RpcError> {
        let dir = self.inner.data_dir.join(crate::plugins::TEMPLATES_DIR);
        let wanted = id.to_owned();
        let here = tokio::task::spawn_blocking(move || template_packages::from_store(&dir).into_iter().any(|(_, store)| store.id == wanted)).await.unwrap_or(false);
        if here { Ok(()) } else { Err(failed(format!("There's no template {} from the store here", plain(id, 64)))) }
    }

    /// What's installed here of a kind, for the store's pages to say.
    async fn installed_of(&self, kind: Kind) -> InstalledOf {
        if kind == Kind::Plugin {
            self.rescan().await;
            return InstalledOf::Plugins;
        }
        let dir = self.inner.data_dir.join(crate::plugins::TEMPLATES_DIR);
        let rows = tokio::task::spawn_blocking(move || -> BTreeMap<String, Value> {
            let mut rows = BTreeMap::new();
            for (folder, store) in template_packages::from_store(&dir) {
                // (Installed twice: the one in the folder of its id.)
                if folder == store.id || !rows.contains_key(&store.id) {
                    let mut row = crate::plugins::templates::installed_row(&dir, &folder);
                    if let Some(update) = row["imported"]["hash"].as_str().and_then(|hash| template_packages::shown_update(&dir, &folder, hash)) {
                        row["update"] = update;
                    }
                    rows.insert(store.id.clone(), row);
                }
            }
            rows
        })
        .await
        .unwrap_or_default();
        InstalledOf::Templates(rows)
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
                "Only a plugin you're making on this device can be published from here (a folder anywhere publishes with `zeron plugin publish <folder>` and a deploy token)",
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
                reply["url"] = json!(client.published_page(id, &reply));
                reply
            }
            Err((error, fields)) => refused(&error, &fields),
        }
    }

    /// Template `id`, one of your own here, read and checked for publishing:
    /// not Zeron's, not one installed from a package, nor one (or a copy of
    /// one) that was taken down.
    async fn own_template(&self, id: &str) -> Result<publish::PreparedTemplate, String> {
        let (dir, id) = (self.inner.data_dir.join(crate::plugins::TEMPLATES_DIR), id.to_owned());
        tokio::task::spawn_blocking(move || {
            let folder = crate::plugins::templates::own_dir(&dir, &id)?;
            template_blocks::refuse(&dir, &folder, "published")?;
            let prepared = publish::read_template(&folder)?;
            // (Here a template is its folder's id; published elsewhere, its
            // manifest's: one that says both is no surprise.)
            if prepared.id() != id {
                return Err(failure(crate::plugins::listing::rule::BAD_PACKAGE, format!("Its zeron-template.json says its id is {:?}: make that {id:?}, its folder's name", plain(prepared.id(), 80))));
            }
            Ok(prepared)
        })
        .await
        .map_err(|_| "Couldn't read it".to_string())?
    }

    /// Your template `id`, as checked (`hash`), signed with this device's key
    /// in the templates' domain and sent with your session: the store's
    /// answer.
    async fn publish_template(&self, id: &str, hash: &str) -> Value {
        let (client, caller) = match self.store_as_person().await {
            Ok(store) => store,
            Err(error) => return not_template(&error),
        };
        let client = client.of(Kind::Template);
        self.register_key().await;
        // (Read again: what's sent is what was checked.)
        let prepared = match self.own_template(id).await {
            Ok(prepared) => prepared,
            Err(error) => return not_template(&error),
        };
        if prepared.hash != hash {
            return not_template(&failure("changed", "It changed since it was checked: check it again"));
        }
        if !prepared.ok() {
            let first = &prepared.problems[0];
            let mut value = refused(&failure(first.rule, publish::lines(&prepared.problems).join("; ")), &json!({ "field": first.field, "cause": first.cause, "word": first.word }));
            says_kind(&mut value, Kind::Template);
            return value;
        }
        let (item, data_dir) = (prepared.id().to_owned(), self.inner.data_dir.clone());
        let packed = tokio::task::spawn_blocking(move || prepared.pack(Some(&signing::load_or_create_key(&data_dir)?))).await;
        let raw = match packed {
            Ok(Ok(raw)) => raw,
            Ok(Err(error)) => return not_template(&error),
            Err(_) => return not_template("Couldn't pack it"),
        };
        let mut reply = match client.publish(&item, raw, &caller).await {
            Ok(mut reply) => {
                reply["ok"] = json!(true);
                *lock(&self.inner.publishes_checked) = None;
                reply["url"] = json!(client.published_page(&item, &reply));
                reply
            }
            Err((error, fields)) => refused(&error, &fields),
        };
        says_kind(&mut reply, Kind::Template);
        reply
    }

    /// A deploy token for the account (contract a315eef5a), its scope fixed
    /// now, made by the edge with a step-up begun for it: its secret, in
    /// the answer this once, never kept or logged here.
    async fn create_deploy_token(&self, p: CreateTokenParams) -> Value {
        let scope = match p.scope.scope() {
            Ok(scope) => scope,
            Err(error) => return not_ok(&failure("invalid_scope", error)),
        };
        let name = plain(&p.name, 64);
        if name.is_empty() {
            return not_ok(&failure("invalid", "Give the token a name (where it's used, like \"GitHub Actions\")"));
        }
        let (client, caller) = match self.store_as_person().await {
            Ok(store) => store,
            Err(error) => return not_ok(&error),
        };
        // A token publishes what you have in the store already, a plugin or
        // a template (a first version comes from a signed-in device): said
        // here, by id, before the step-up.
        let mine = match client.my_plugins(&caller).await {
            Ok(mine) => mine,
            Err(error) => return not_ok(&error),
        };
        let mine_templates = match client.of(client::Kind::Template).my_plugins(&caller).await {
            Ok(mine) => mine,
            // (A store without templates has none of yours.)
            Err(error) if install::reason_of(&error) == Some(client::reason::NOT_IN_STORE) => Value::Null,
            Err(error) => return not_ok(&error),
        };
        let ids = |mine: &Value, key: &str| -> Vec<String> { mine[key].as_array().into_iter().flatten().filter_map(|item| item["id"].as_str().map(str::to_owned)).collect() };
        let mut owned = ids(&mine, "plugins");
        owned.extend(ids(&mine_templates, "templates"));
        if owned.is_empty() {
            return not_ok(&failure(
                "not_published",
                "You have no plugin or template in the store yet: publish one from this device, then make a deploy token",
            ));
        }
        if let Scope::Plugins(ids) = &scope {
            let missing: Vec<&str> = ids.iter().filter(|id| !owned.contains(id)).map(String::as_str).collect();
            if !missing.is_empty() {
                return not_owner(&missing);
            }
        }
        // A step-up begun for this (`StepUp {for: "deploy_token"}`), used up
        // here: a sign-in's ticket, or one already spent, isn't one.
        let step_up = lock(&self.inner.deploy_token_step_up).clone().and_then(|f| f());
        let Some(step_up) = step_up else {
            return not_ok(&failure("step_up_required", "Making a deploy token needs a fresh sign-in for it"));
        };
        let mut body = json!({ "name": name, "scope": scope.to_value(), "stepUp": step_up });
        if let Some(expires) = p.expires {
            body["expires"] = json!(expires);
        }
        // The token, read as a token is (else nothing usable came back), and
        // its record as the list shows it; the copies read are wiped, and
        // the one copy left goes to the app, which shows it once.
        let (token, secret) = match client.create_token(body, &caller).await {
            Ok(made) => made,
            Err(error) => return not_ok(&error),
        };
        tracing::info!(short = %secret.short(), "store: a deploy token was made");
        json!({ "ok": true, "token": token, "secret": secret.as_str() })
    }

    /// One file of a store version, from this device's cache or fetched
    /// into it.
    async fn store_file(&self, kind: Kind, p: FileParams) -> Result<Value, String> {
        Version::parse(&p.version)?;
        let manifest_file = match kind {
            Kind::Plugin => manifest::MANIFEST_FILE,
            Kind::Template => crate::plugins::templates::MANIFEST,
        };
        if !package::plain_path(&p.path) || p.path == manifest_file {
            return Err(format!("That isn't a file in a {}", if kind == Kind::Plugin { "plugin" } else { "template" }));
        }
        // (Apart by kind: a plugin's call never answers a template's file. No
        // id starts with a dot.)
        let cache = match kind {
            Kind::Plugin => self.inner.data_dir.join(CACHE_DIR).join(&p.id),
            Kind::Template => self.inner.data_dir.join(CACHE_DIR).join(".templates").join(&p.id),
        };
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
        let bytes = client.of(kind).file(&p.id, &p.version, &p.path, &caller, MAX_FILE_BYTES).await?;
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
    /// any install's) asks `added` and `removed` against `asked`. A
    /// pre-release only where this device takes them of it. None when
    /// there's none, or the store can't say.
    async fn verified_alternative(&self, client: &StoreClient, caller: &Caller, id: &str, asked: &str, asks: &[String], publisher: &client::Publisher) -> Option<install::Alternative> {
        let zeron = version::zeron();
        let prereleases = lock(&self.inner.state).prerelease.contains(id);
        let mut cursor: Option<String> = None;
        for _ in 0..ALTERNATIVE_PAGES {
            let page = client.versions(id, cursor.as_deref(), Some(50), caller).await.ok()?;
            let found = page["versions"].as_array().into_iter().flatten().find(|v| {
                v["verified"] == true
                    && v["status"] == "listed"
                    && v["yanked"] != true
                    && v["version"].as_str().is_some_and(|x| !same_version(x, asked))
                    && v["version"].as_str().is_some_and(|x| prereleases || Version::parse(x).is_ok_and(|x| !x.is_prerelease()))
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
    /// can't be read, names other content or another signer than `signer`
    /// (the package's own: none, unsigned), is gone, or isn't installable
    /// (yanked, superseded, not listed) refuses the update: it never fails
    /// open.
    pub(in crate::plugins) async fn verified_now(&self, id: &str, version: &str, hash: &str, signer: Option<&str>) -> Result<bool, String> {
        let Ok((client, caller)) = self.store_reader() else { return Ok(false) };
        match client.version(id, version, &caller).await {
            Ok((_, record)) if record.status != "listed" || record.yanked => {
                Err(failure(client::reason::NOT_IN_STORE, format!("Version {version} of {id} can't be installed any more")))
            }
            Ok((_, record)) if record.hash.eq_ignore_ascii_case(hash) && record.key.as_deref() == signer => Ok(record.verified),
            Ok(_) => Err(failure(client::reason::STORE_MISMATCH, format!("The store's record of {id} {version} names other content or another signer than the update found, so it wasn't taken"))),
            Err(error) if install::reason_of(&error) == Some(client::reason::STORE_UNREACHABLE) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// `ImportTemplate {store, version?}`: template `id`'s version from the
    /// store (none named: its newest listed, for a person; its newest
    /// verified, for an agent), fetched and checked against its record, to
    /// wait for the person in Zeron's own window, whatever its record says.
    pub(in crate::plugins) async fn import_template_from_store(&self, id: &str, version: Option<&str>) -> Result<Value, RpcError> {
        let id = id.trim();
        check_item_id(Kind::Template, id)?;
        let person = Self::by_person();
        let (client, caller) = self.store_reader()?;
        let client = client.of(Kind::Template);
        let (_, detail) = client.plugin(id, &caller).await.map_err(failed)?;
        let named = version.map(str::trim).filter(|v| !v.is_empty());
        let wanted = match (named, person) {
            (Some(named), _) => named.to_owned(),
            (None, true) => detail.version.clone(),
            (None, false) => detail.verified_version.clone().ok_or_else(|| agent_unverified(id, &detail.version))?,
        };
        if wanted.is_empty() {
            return Err(failed(failure(client::reason::NOT_IN_STORE, format!("The store lists no version of {id}"))));
        }
        let (package, source) = self.fetch_template(&client, &caller, id, &wanted, &detail.author).await?;
        if !person && !source.store().is_some_and(|s| s.verified) {
            return Err(agent_unverified(id, &wanted));
        }
        self.stage_template(&client, &caller, package, source, &detail.author, None, false).await
    }

    /// `InstallVerifiedTemplate {pending, hash}`: the verified version a
    /// waiting template's sheet offers instead of it, its own item's (never a
    /// caller's word), its record read now and refused (`not_verified`)
    /// unless it says so. It waits in that one's place.
    pub(in crate::plugins) async fn install_verified_template(&self, pending: &str, hash: &str) -> Result<Value, RpcError> {
        if !Self::by_person() {
            return Err(failed(failure(install::reason::NEEDS_PERSON, "Only the person chooses another version, in Zeron's window")));
        }
        let waiting = self.waiting_from_store(pending).await?.ok_or_else(|| failed("There's no template from the store waiting by that name"))?;
        if !waiting.hash.eq_ignore_ascii_case(hash.trim()) {
            return Err(failed("It changed since it was shown: look again"));
        }
        let id = waiting.store.id.clone();
        let Some(version) = waiting.store.verified_alternative.as_ref().filter(|_| !waiting.store.verified).map(|a| a.version.clone()) else {
            return Err(failed(failure(install::reason::NOT_VERIFIED, format!("{id} has no verified version to install instead"))));
        };
        let (client, caller) = self.store_reader()?;
        let client = client.of(Kind::Template);
        let (_, detail) = client.plugin(&id, &caller).await.map_err(failed)?;
        let (package, source) = self.fetch_template(&client, &caller, &id, &version, &detail.author).await?;
        if !source.store().is_some_and(|s| s.verified) {
            return Err(failed(failure(install::reason::NOT_VERIFIED, format!("The store's record of {id} {version} doesn't say it's verified, so it wasn't taken"))));
        }
        let staged = self.stage_template(&client, &caller, package, source, &detail.author, waiting.update_of, true).await?;
        let (dir, was) = (self.inner.data_dir.join(crate::plugins::TEMPLATES_DIR), pending.to_owned());
        if staged["pending"] != was.as_str() {
            let _ = tokio::task::spawn_blocking(move || template_packages::drop_waiting(&dir, &was)).await;
        }
        Ok(staged)
    }

    /// Version `version` of template `id`, by its own record, fetched now and
    /// listed; its package fetched and checked against it.
    async fn fetch_template(&self, client: &StoreClient, caller: &Caller, id: &str, version: &str, publisher: &client::Publisher) -> Result<(template_packages::Package, Source), RpcError> {
        Version::parse(version).map_err(failed)?;
        let (_, found) = client.version(id, version, caller).await.map_err(failed)?;
        if found.status != "listed" || found.yanked {
            return Err(failed(failure(client::reason::NOT_IN_STORE, format!("Version {version} of {id} can't be installed any more"))));
        }
        let record = PackageRecord { hash: found.hash, sha256: found.sha256, bytes: found.bytes, key: found.key };
        let raw = client.package(id, version, caller, record.bytes).await.map_err(failed)?;
        let (id, version, publisher, verified) = (id.to_owned(), version.to_owned(), publisher.clone(), found.verified);
        let named = version.clone();
        let (package, mut store) = tokio::task::spawn_blocking(move || super::verify_template(raw, &id, &named, &record, &publisher, verified))
            .await
            .map_err(|_| failed("Couldn't read it"))?
            .map_err(failed)?;
        store.verified_at = found.verified.then_some(found.verified_at).flatten();
        Ok((package, Source { kind: STORE.into(), url: None, path: None, git_ref: None, commit: None, version, installed_at: now_secs(), store: Some(store) }))
    }

    /// A template fetched from the store, waiting; not verified, with the
    /// newest verified version this Zeron can run, for its sheet's "instead".
    async fn stage_template(&self, client: &StoreClient, caller: &Caller, package: template_packages::Package, mut source: Source, publisher: &client::Publisher, at: Option<String>, instead: bool) -> Result<Value, RpcError> {
        let dir = self.inner.data_dir.join(crate::plugins::TEMPLATES_DIR);
        if let Some(store) = source.store.as_mut().filter(|s| !s.verified) {
            let place = {
                let (dir, id, at) = (dir.clone(), store.id.clone(), at.clone());
                tokio::task::spawn_blocking(move || template_packages::place_of(&dir, &id, at.as_deref())).await.ok().flatten()
            };
            let prereleases = lock(&self.inner.state).template_prerelease.contains(&store.id);
            store.verified_alternative = verified_template(client, caller, &store.id.clone(), &source.version, publisher, place.as_ref(), prereleases).await;
        }
        let staged = tokio::task::spawn_blocking(move || template_packages::stage_from_store(&dir, &package, source, at.as_deref(), instead))
            .await
            .map_err(|_| failed("Couldn't reach the templates folder"))?
            .map_err(failed)?;
        tracing::info!(id = %staged["id"], version = %staged["version"], "store: a template waits to be allowed");
        Ok(staged)
    }

    /// The store template in folder `id`, if that's what it is.
    pub(in crate::plugins) async fn store_template_at(&self, id: &str) -> Option<template_packages::Installed> {
        let (dir, id) = (self.inner.data_dir.join(crate::plugins::TEMPLATES_DIR), id.to_owned());
        tokio::task::spawn_blocking(move || template_packages::installed_from_store(&dir, Some(&id)).into_iter().next()).await.ok()?
    }

    /// `UpdateTemplate` for a store template: the person's, its update's
    /// record read now; it waits for them but for its listing files alone.
    pub(in crate::plugins) async fn update_store_template(&self, installed: template_packages::Installed) -> Result<Value, RpcError> {
        if !Self::by_person() {
            return Err(Self::needs_person(&installed.name));
        }
        let dir = self.inner.data_dir.join(crate::plugins::TEMPLATES_DIR);
        let found = |installed: &template_packages::Installed| {
            let (dir, id, hash) = (dir.clone(), installed.id.clone(), installed.hash.clone());
            async move { tokio::task::spawn_blocking(move || template_packages::found(&dir, &id, &hash)).await.ok().flatten() }
        };
        let mut kept = found(&installed).await;
        if kept.is_none() {
            let (_, errors) = self.check_store_templates(Some(&installed.id)).await;
            if let Some(error) = errors.first() {
                return Err(failed(error["error"].as_str().unwrap_or("Couldn't check for an update").to_owned()));
            }
            kept = found(&installed).await;
        }
        let Some(kept) = kept else {
            return Err(failed(install::failure(install::reason::UP_TO_DATE, format!("{} is up to date", installed.name))));
        };
        if let Some(refused) = kept.refused {
            return Err(failed(refused));
        }
        let unverified = || failed(failure(install::reason::NOT_VERIFIED, format!("{} {} isn't verified, and this device takes verified updates of it", installed.name, plain(&kept.version, 64))));
        if kept.held {
            return Err(unverified());
        }
        let store = kept.source.as_ref().and_then(Source::store).cloned().ok_or_else(|| failed(template_packages::NOT_CHECKED))?;
        let waiting = template_packages::FromStore { update_of: Some(installed.id.clone()), content_hash: store.hash.clone(), version: kept.version.clone(), hash: kept.hash.clone(), store };
        let now = self.record_now(&waiting).await?;
        // (A device that takes verified updates takes nothing its record
        // doesn't say is verified, now.)
        if !now.verified && lock(&self.inner.state).template_updates(&installed.store.id) == UpdatePreference::Verified {
            return Err(unverified());
        }
        // Not verified: the verified version its sheet can offer instead.
        let mut alternative = None;
        if !now.verified && let Ok((client, caller)) = self.store_reader() {
            let prereleases = lock(&self.inner.state).template_prerelease.contains(&now.id);
            let publisher = client::Publisher { id: Some(now.publisher_id.clone()), username: Some(now.publisher.clone()), verified: false };
            let place = template_packages::Place { folder: installed.id.clone(), version: installed.version.clone(), verified: installed.store.verified };
            alternative = verified_template(&client.of(Kind::Template), &caller, &now.id, &waiting.version, &publisher, Some(&place), prereleases).await;
        }
        let id = installed.id;
        tokio::task::spawn_blocking(move || template_packages::update_from_store(&dir, &id, &now, alternative)).await.map_err(|_| failed("Couldn't reach the templates folder"))?.map_err(failed)
    }

    /// `change` what updates this device takes. What was found for a plugin
    /// or a template by another choice than its own now is forgotten: the
    /// next check, or its update, looks by this one.
    async fn choose_updates(&self, change: impl FnOnce(&mut State)) -> Result<(), RpcError> {
        self.ensure_scanned().await;
        // (In the one lock: two at once each see what the other chose.)
        let mut before = State::default();
        let saved = self.update_state(|state| {
            before = state.clone();
            change(state);
        });
        let plugins: Vec<String> = {
            let now = lock(&self.inner.state).clone();
            let entries = lock(&self.inner.entries);
            entries.iter().filter(|e| e.store.is_some() && before.choice(&e.item.id) != now.choice(&e.item.id)).map(|e| e.item.id.clone()).collect()
        };
        let (this, dir) = (self.clone(), self.inner.data_dir.join(crate::plugins::TEMPLATES_DIR));
        let forgotten = !plugins.is_empty();
        tokio::task::spawn_blocking(move || {
            {
                let _finding = install::finding();
                for id in &plugins {
                    install::forget_found(&this.inner.plugins_dir, id);
                }
            }
            let _changing = template_packages::changing();
            let now = lock(&this.inner.state).clone();
            for (folder, store) in template_packages::from_store(&dir) {
                if before.template_choice(&store.id) != now.template_choice(&store.id) {
                    template_packages::forget_found(&dir, &folder);
                }
            }
        })
        .await
        .map_err(|_| failed("Couldn't reach the templates folder"))?;
        if forgotten {
            self.rescan().await;
        }
        saved
    }

    /// Whether this device still takes what a check of `c` looked by.
    fn chosen_still(&self, c: &Candidate) -> Result<(), String> {
        match lock(&self.inner.state).choice(&c.id) == (c.prefer, c.prerelease) {
            true => Ok(()),
            false => Err(CHOSEN_ANEW.into()),
        }
    }

    /// Keep what a check by `choice` found for store template `t` (none:
    /// nothing), unless another was made since.
    async fn keep_template_found(&self, t: &template_packages::Installed, choice: (UpdatePreference, bool), found: Option<Found>, package: Option<template_packages::Package>) -> Result<(), String> {
        let (this, dir) = (self.clone(), self.inner.data_dir.join(crate::plugins::TEMPLATES_DIR));
        let (id, store_id) = (t.id.clone(), t.store.id.clone());
        tokio::task::spawn_blocking(move || {
            let _changing = template_packages::changing();
            if lock(&this.inner.state).template_choice(&store_id) != choice {
                return Err(CHOSEN_ANEW.into());
            }
            match found {
                Some(found) => template_packages::keep_found(&dir, &id, &found, package.as_ref()),
                None => {
                    template_packages::forget_found(&dir, &id);
                    Ok(())
                }
            }
        })
        .await
        .map_err(|_| "Couldn't keep the update".to_string())?
    }

    /// The update check of the store's templates here (`only`: the one in
    /// that folder): each one's update by this device's choice, its package
    /// kept, never installed: `(updates, errors)`, as a plugin's check says
    /// them, `id` the folder's. (What's taken down is the feed's and the full
    /// check's to say.)
    pub(in crate::plugins) async fn check_store_templates(&self, only: Option<&str>) -> (Vec<Value>, Vec<Value>) {
        let dir = self.inner.data_dir.join(crate::plugins::TEMPLATES_DIR);
        let here = {
            let (dir, only) = (dir.clone(), only.map(str::to_owned));
            tokio::task::spawn_blocking(move || template_packages::installed_from_store(&dir, only.as_deref())).await.unwrap_or_default()
        };
        let said = |t: &template_packages::Installed, error: &str| json!({ "id": t.id, "name": t.name, "error": error });
        if here.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let (client, caller) = match self.store_reader() {
            Ok(store) => store,
            Err(error) => return (Vec::new(), here.iter().map(|t| said(t, &error.to_string())).collect()),
        };
        let client = client.of(Kind::Template);
        let state = lock(&self.inner.state).clone();
        // Each version of each, once, an id once in a request.
        let mut all: Vec<template_blocks::Identity> = here.iter().map(|t| template_blocks::Identity { id: t.store.id.clone(), version: t.version.clone(), hash: t.store.hash.clone() }).filter(template_blocks::Identity::valid).collect();
        all.sort();
        all.dedup();
        let (mut updates, mut errors) = (Vec::new(), Vec::new());
        let mut answered: Vec<(template_blocks::Identity, Option<Offer>)> = Vec::new();
        for asked in template_blocks::requests(&all, MAX_UPDATE_ITEMS) {
            let ask = UpdatesAsk {
                zeron: version::zeron().to_string(),
                plugins: Vec::new(),
                templates: asked.iter().map(|t| Asked { id: t.id.clone(), version: t.version.clone(), hash: t.hash.clone(), prerelease: state.template_prerelease.contains(&t.id) }).collect(),
            };
            match client.updates(&ask, &caller).await {
                Ok(reply) => {
                    for item in asked {
                        if reply.unreadable.iter().any(|id| id.as_deref().is_none_or(|id| id == item.id)) {
                            errors.extend(here.iter().filter(|t| t.store.id == item.id && t.version == item.version).map(|t| said(t, &failure(client::reason::STORE_ERROR, "The store's answer about it couldn't be read"))));
                            continue;
                        }
                        let offer = reply.template_updates.iter().find(|o| o.id == item.id).cloned();
                        answered.push((item, offer));
                    }
                }
                Err(error) => errors.extend(here.iter().filter(|t| asked.iter().any(|a| a.id == t.store.id && a.version == t.version)).map(|t| said(t, &error))),
            }
        }
        for t in &here {
            let Some((_, offer)) = answered.iter().find(|(asked, _)| asked.id == t.store.id && asked.version == t.version && asked.hash == t.store.hash) else { continue };
            let offer = offer.as_ref().filter(|o| install::newer_by_precedence(&o.version, &t.version));
            let choice = state.template_choice(&t.store.id);
            let (staged, beside) = match (offer, choice.0) {
                (None, _) => (None, None),
                (Some(offer), UpdatePreference::Newest) => (Some(offer), offer.verified_update.as_deref()),
                (Some(offer), UpdatePreference::Verified) if offer.verified => (Some(offer), None),
                (Some(offer), UpdatePreference::Verified) => (offer.verified_update.as_deref().filter(|v| v.verified && install::newer_by_precedence(&v.version, &t.version)), None),
            };
            let Some(staged) = staged else {
                // Only unverified updates, and this device takes verified
                // ones of it: said, not kept.
                let held = offer.map(|offer| Found {
                    from: t.hash.clone(),
                    version: plain(&offer.version, version::MAX_LEN),
                    installed_verified: Some(t.store.verified),
                    held: true,
                    ..Found::default()
                });
                if let Err(error) = self.keep_template_found(t, choice, held.clone(), None).await {
                    errors.push(said(t, &error));
                    continue;
                }
                if let Some(held) = held {
                    let mut shown = held.shown();
                    shown["id"] = json!(t.id);
                    shown["name"] = json!(t.name);
                    updates.push(shown);
                }
                continue;
            };
            match self.found_template(&client, &caller, t, choice, staged, beside).await {
                Ok(mut shown) => {
                    shown["id"] = json!(t.id);
                    shown["name"] = json!(t.name);
                    updates.push(shown);
                }
                Err(error) => errors.push(said(t, &error)),
            }
        }
        (updates, errors)
    }

    /// `offer` as the update found for store template `t`: its package
    /// fetched by its own record (or the one kept, when it's that), checked
    /// and kept.
    async fn found_template(&self, client: &StoreClient, caller: &Caller, t: &template_packages::Installed, choice: (UpdatePreference, bool), offer: &Offer, beside: Option<&Offer>) -> Result<Value, String> {
        let dir = self.inner.data_dir.join(crate::plugins::TEMPLATES_DIR);
        let kept = {
            let (dir, id, hash) = (dir.clone(), t.id.clone(), t.hash.clone());
            tokio::task::spawn_blocking(move || {
                let found = template_packages::found(&dir, &id, &hash)?;
                let package = template_packages::kept_from_store(&dir, &id, &found).ok()?;
                Some((package, found.source?))
            })
            .await
            .ok()
            .flatten()
        };
        // (Its mark and publisher, as the offer says them: for its sheet;
        // taking it asks the store's record again.)
        let (publisher, publisher_id) = super::vouched(&offer.publisher())?;
        let (package, mut source) = match kept.filter(|(package, _)| package.content_hash == offer.hash && package.version() == offer.version && package.key == offer.key) {
            Some(kept) => kept,
            None => self.fetch_template(client, caller, &t.store.id, &offer.version, &offer.publisher()).await.map_err(|e| e.to_string())?,
        };
        if package.content_hash != offer.hash {
            return Err(failure(client::reason::STORE_MISMATCH, "The store's offer and its record name other contents, so it wasn't kept"));
        }
        if let Some(store) = source.store.as_mut() {
            store.publisher = publisher;
            store.publisher_id = publisher_id;
            store.verified = offer.verified;
            store.verified_at = offer.verified.then_some(offer.verified_at).flatten();
        }
        let refused = if install::newer_by_precedence(&package.version(), &t.version) { package.compatible().err() } else { Some(install::not_newer(&package.version(), &t.version)) };
        let found = Found {
            from: t.hash.clone(),
            version: package.version(),
            hash: package.hash.clone(),
            refused,
            new_owner: source.store().is_some_and(|s| s.publisher_id != t.store.publisher_id),
            source: Some(source),
            notes: notes(&offer.notes, listing::MAX_NOTES_BYTES),
            store_verified: Some(offer.verified),
            store_verified_at: offer.verified.then_some(offer.verified_at).flatten(),
            installed_verified: Some(t.store.verified),
            verified_alternative: beside.filter(|b| b.verified && !offer.verified && install::newer_by_precedence(&b.version, &t.version)).map(|b| install::Alternative {
                version: plain(&b.version, version::MAX_LEN),
                added: Vec::new(),
                removed: Vec::new(),
                notes: notes(&b.notes, listing::MAX_NOTES_BYTES),
                verified_at: b.verified_at,
            }),
            ..Found::default()
        };
        let shown = found.shown();
        let keep = found.refused.is_none().then_some(package);
        self.keep_template_found(t, choice, Some(found), keep).await?;
        Ok(shown)
    }

    /// A store template installed here, anew, is counted once per account
    /// (the edge's to say).
    pub(in crate::plugins) fn template_installed(&self, store: &install::StoreSource) {
        let Ok((client, caller)) = self.store_reader() else { return };
        let (client, id) = (client.of(Kind::Template), store.id.clone());
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Err(error) = client.installed(&id, &caller).await {
                    tracing::debug!(template = %id, %error, "store: the install wasn't counted");
                }
            });
        }
    }

    /// The waiting template package `pending`, if it's one from the store.
    pub(in crate::plugins) async fn waiting_from_store(&self, pending: &str) -> Result<Option<template_packages::FromStore>, RpcError> {
        let (dir, pending) = (self.inner.data_dir.join(crate::plugins::TEMPLATES_DIR), pending.to_owned());
        tokio::task::spawn_blocking(move || template_packages::waiting_from_store(&dir, &pending)).await.map_err(|_| failed("Couldn't reach the templates folder"))?.map_err(failed)
    }

    /// The store's record of a waiting template, asked now: its mark and its
    /// publisher. Refused when the store can't be asked (`NOT_CHECKED`), when
    /// it's taken down (`taken_down`) or no longer listed (`not_in_store`),
    /// and when its record is of other contents or another signer.
    pub(in crate::plugins) async fn record_now(&self, waiting: &template_packages::FromStore) -> Result<install::StoreSource, RpcError> {
        let (was, version) = (&waiting.store, waiting.version.as_str());
        let dir = self.inner.data_dir.join(crate::plugins::TEMPLATES_DIR);
        let identity = template_blocks::Identity { id: was.id.clone(), version: version.to_owned(), hash: was.hash.clone() };
        let (content_hash, known) = (waiting.content_hash.clone(), identity.clone());
        tokio::task::spawn_blocking(move || template_blocks::refuse_package(&dir, &content_hash, Some(&known))).await.map_err(|_| failed(template_packages::NOT_CHECKED))?.map_err(failed)?;
        let (client, caller) = self.store_reader().map_err(|_| failed(template_packages::NOT_CHECKED))?;
        let client = client.of(client::Kind::Template);
        let gone = || failure(client::reason::NOT_IN_STORE, "This version of the template can't be installed from the store any more");
        let refused = |error: String| match install::reason_of(&error) {
            Some(client::reason::NOT_IN_STORE) => gone(),
            Some(client::reason::STORE_MISMATCH) => error,
            _ => template_packages::NOT_CHECKED.to_owned(),
        };
        let record = match client.version(&was.id, version, &caller).await {
            Ok((_, record)) if record.status == "listed" && !record.yanked => record,
            Ok(_) => return Err(failed(gone())),
            Err(error) => {
                let error = refused(error);
                if install::reason_of(&error) == Some(client::reason::NOT_IN_STORE)
                    && let Some(reason) = self.taken_down(&client, &caller, &identity).await
                {
                    let why = if reason.is_empty() { String::new() } else { format!(" ({reason})") };
                    return Err(failed(failure(template_blocks::TAKEN_DOWN, format!("This template was taken down from the store{why}, so it wasn't installed."))));
                }
                return Err(failed(error));
            }
        };
        if !record.hash.eq_ignore_ascii_case(&was.hash) || record.key != was.key {
            return Err(failed(failure(client::reason::STORE_MISMATCH, format!("The store's record of {} {version} names other content or another signer, so it wasn't installed", was.id))));
        }
        let (_, detail) = client.plugin(&was.id, &caller).await.map_err(|e| failed(refused(e)))?;
        let (publisher, publisher_id) = super::vouched(&detail.author).map_err(failed)?;
        Ok(install::StoreSource { publisher, publisher_id, verified: record.verified, verified_at: record.verified.then_some(record.verified_at).flatten(), ..was.clone() })
    }

    /// Why the store took `identity` down, if it did (a version gone from its
    /// pages may only be yanked, or removed).
    async fn taken_down(&self, client: &StoreClient, caller: &Caller, identity: &template_blocks::Identity) -> Option<String> {
        let ask = UpdatesAsk {
            zeron: version::zeron().to_string(),
            plugins: Vec::new(),
            templates: vec![Asked { id: identity.id.clone(), version: identity.version.clone(), hash: identity.hash.clone(), prerelease: false }],
        };
        let reply = client.updates(&ask, caller).await.ok()?;
        let block = reply.blocked.into_iter().find(|b| b.item_kind == Some(client::Kind::Template) && b.id == identity.id && same_version(&b.version, &identity.version))?;
        Some(plain(&block.reason, 500))
    }

    /// Whether a waiting store template is still what its window draws
    /// (`shown`), by its record `now`: the state its review is good for, or
    /// `{changed}`. Refused when the window says nothing of it, or draws
    /// another package.
    pub(in crate::plugins) async fn shown_still(
        &self,
        pending: &str,
        waiting: &template_packages::FromStore,
        hash: &str,
        shown: Option<&ShownRecord>,
        now: &install::StoreSource,
    ) -> Result<Result<String, Value>, RpcError> {
        let shown = shown.ok_or_else(|| failed("Say what's shown of it (`shown`)"))?;
        if waiting.hash != hash || listing::shown(&waiting.version, version::MAX_LEN) != shown.version {
            return Err(failed("This template changed while it waited for you. Look at it again."));
        }
        let drawn = (shown.verified, shown.publisher.as_str(), shown.publisher_id.as_str());
        let publisher = template_packages::shown_publisher(now);
        if drawn == (now.verified, publisher.as_str(), now.publisher_id.as_str()) {
            return Ok(Ok(record_state(now)));
        }
        Ok(Err(self.record_changed(pending, now).await?))
    }

    /// `{changed}`: what the store's record of the waiting template says now,
    /// kept for the list to say too.
    pub(in crate::plugins) async fn record_changed(&self, pending: &str, now: &install::StoreSource) -> Result<Value, RpcError> {
        let (dir, name, record) = (self.inner.data_dir.join(crate::plugins::TEMPLATES_DIR), pending.to_owned(), now.clone());
        tokio::task::spawn_blocking(move || template_packages::keep_record(&dir, &name, &record)).await.map_err(|_| failed("Couldn't reach the templates folder"))?.map_err(failed)?;
        let mut changed = json!({ "verified": now.verified, "publisher": template_packages::shown_publisher(now), "publisher_id": now.publisher_id });
        if let Some(at) = now.verified_at {
            changed["verified_at"] = json!(at);
        }
        Ok(json!({ "changed": changed }))
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
                    key: e.key().map(str::to_owned).or_else(|| store.last_key.clone()),
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
        let templates_dir = self.inner.data_dir.join(crate::plugins::TEMPLATES_DIR);
        let (has_templates, lost) = {
            let dir = templates_dir.clone();
            tokio::task::spawn_blocking(move || (!template_blocks::installed(&dir).is_empty(), template_blocks::wanted(&dir))).await.unwrap_or((true, true))
        };
        if self.store_candidates(None).is_empty() && !has_templates {
            return;
        }
        let Ok((client, caller)) = self.store_reader() else { return };
        let mut after = lock(&self.inner.state).takedowns_after;
        // What's blocked of templates is lost: it starts over, whole.
        if lost {
            after = None;
        }
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
                let check = self.check_store(None).await;
                if check.errors.is_empty() && check.templates_settled {
                    self.end_marks(&client, &caller).await;
                    let _ = self.update_state(|state| state.takedowns_after = Some(feed.head));
                }
                return;
            }
            let events: Vec<_> = feed.events.iter().filter(|e| after.is_none_or(|a| e.seq > a) && e.seq <= feed.head).collect();
            let Some(last) = events.iter().map(|e| e.seq).max() else { return };
            // Templates' first: a place in the feed is past both kinds. One
            // that couldn't be kept is asked for again, from a fresh start
            // when what's kept can't be read.
            let changes: Vec<template_blocks::Change> = events
                .iter()
                .filter(|e| e.item_kind == Some(client::Kind::Template))
                .filter_map(|e| match e.kind.as_str() {
                    "take_down" => Some(template_blocks::Change::TakenDown(template_blocks::Block {
                        id: e.id.clone(),
                        version: e.version.clone(),
                        hash: e.hash.clone(),
                        reason: plain(&e.reason, 500),
                        at: now_secs(),
                    })),
                    "restore" => Some(template_blocks::Change::Restored(e.id.clone(), e.version.clone())),
                    _ => None,
                })
                .collect();
            let templates_kept = match self.apply_template_changes(&templates_dir, changes).await {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(%error, "store: a template's take-down couldn't be kept");
                    if template_blocks::read(&templates_dir).is_err() {
                        let _ = self.update_state(|state| state.takedowns_after = None);
                    }
                    false
                }
            };
            // (A template's is never a plugin's here.)
            let events = events.into_iter().filter(|e| e.item_kind == Some(client::Kind::Plugin));
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
            // (A template's that couldn't be kept is asked for again; the
            // plugins' were applied, and applying one again changes nothing.)
            if !templates_kept {
                return;
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
    /// with nothing there is asked again only every few hours.
    pub async fn check_publishes(&self) {
        let Ok((client, caller)) = self.store_as_person().await else { return };
        let idle_for = publishes_idle(super::super::lifecycle::local_every());
        let idle = lock(&self.inner.publishes_checked).is_some_and(|(at, had)| !had && at.elapsed() < idle_for);
        if idle {
            return;
        }
        // Each kind on its own: one that can't be read holds up no other.
        let (plugins, templates) = (client.my_plugins(&caller).await, client.of(Kind::Template).my_plugins(&caller).await);
        let own_key = signing::load_key(&self.inner.data_dir).map(|key| key.public()).unwrap_or_default();
        let (mut had, mut banners) = (false, Vec::new());
        for (kind, answer) in [(Kind::Plugin, plugins), (Kind::Template, templates)] {
            let mine = match answer {
                Ok(mine) => mine,
                // (A store without templates has none of yours.)
                Err(error) if kind == Kind::Template && install::reason_of(&error) == Some(client::reason::NOT_IN_STORE) => continue,
                Err(error) => {
                    tracing::debug!(%error, kind = kind.segment(), "store: the account's publishes couldn't be read");
                    had = true;
                    continue;
                }
            };
            had |= mine[kind.segment()].as_array().is_some_and(|items| !items.is_empty());
            let account = own_publisher_id(&mine).unwrap_or_default();
            let before = match kind {
                Kind::Plugin => lock(&self.inner.state).publishes.clone(),
                Kind::Template => lock(&self.inner.state).template_publishes.clone(),
            }
            .filter(|seen| seen.account == account);
            let (mut seen, new) = notices::look(kind, before.as_ref(), &mine, &own_key, |text| plain(text, 120));
            seen.account = account;
            if before.as_ref() == Some(&seen) && new.is_empty() {
                continue;
            }
            banners.extend(new.iter().map(|n| (if n.template { "Template published" } else { "Plugin published" }, n.words())));
            let _ = self.update_state(|state| {
                match kind {
                    Kind::Plugin => state.publishes = Some(seen),
                    Kind::Template => state.template_publishes = Some(seen),
                }
                notices::add(&mut state.publish_notices, new);
            });
        }
        *lock(&self.inner.publishes_checked) = Some((std::time::Instant::now(), had));
        if banners.is_empty() {
            return;
        }
        self.publish_list();
        if let Some(notify) = lock(&self.inner.notifier).clone() {
            for (title, words) in banners {
                notify("zeron", title, &words, None);
            }
        }
    }

    /// Make `changes` to what's blocked of templates, and say which of those
    /// installed here that newly blocks.
    async fn apply_template_changes(&self, dir: &Path, changes: Vec<template_blocks::Change>) -> Result<(), String> {
        if changes.is_empty() {
            return Ok(());
        }
        let dir = dir.to_owned();
        let _held = self.inner.template_checks.lock().await;
        self.changing_blocks(move || template_blocks::apply(&dir, changes, now_secs())).await
    }

    /// Run `change` (to what's blocked of templates), then tell you of each
    /// template here it newly blocks.
    async fn changing_blocks<T: Send + 'static>(&self, change: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
        let dir = self.inner.data_dir.join(crate::plugins::TEMPLATES_DIR);
        let (changed, newly) = tokio::task::spawn_blocking(move || {
            let blocked = || -> BTreeMap<String, template_blocks::Block> {
                template_blocks::installed(&dir).into_iter().filter_map(|(id, _)| template_blocks::blocked(&dir, &dir.join(&id)).map(|b| (id, b))).collect()
            };
            let before = blocked();
            let changed = change();
            let newly: Vec<(String, template_blocks::Block)> = blocked().into_iter().filter(|(id, _)| !before.contains_key(id)).collect();
            (changed, newly)
        })
        .await
        .map_err(|_| "Couldn't keep it".to_string())?;
        for (folder, block) in newly {
            tracing::warn!(template = %folder, id = %block.id, version = %block.version, "store: a template was blocked");
            let why = if block.reason.is_empty() { String::new() } else { format!(": {}", block.reason) };
            self.key_notice(
                &format!("template_blocked:{folder}:{}:{}", block.id, block.version),
                "template_blocked",
                None,
                "Template blocked",
                &format!("The template {folder} was taken down from the store{why}. Zeron won't run it."),
            );
        }
        changed
    }

    pub(in crate::plugins) async fn check_store_updates(&self, only: Option<&str>) -> (Vec<Value>, Vec<Value>) {
        let check = self.check_store(only).await;
        (check.updates, check.errors)
    }

    async fn check_store(&self, only: Option<&str>) -> StoreCheck {
        let candidates = self.store_candidates(only);
        let templates_dir = self.inner.data_dir.join(crate::plugins::TEMPLATES_DIR);
        // (One plugin's check asks about it alone.)
        let templates: Vec<template_blocks::Identity> = match only {
            Some(_) => Vec::new(),
            None => {
                let dir = templates_dir.clone();
                tokio::task::spawn_blocking(move || template_blocks::all_identities(&dir)).await.unwrap_or_default()
            }
        };
        let mut check = StoreCheck { updates: Vec::new(), errors: Vec::new(), templates_settled: templates.is_empty() };
        if candidates.is_empty() && templates.is_empty() {
            return check;
        }
        let said = |c: &Candidate, error: &str| json!({ "id": c.id, "name": c.name, "error": error });
        let (client, caller) = match self.store_reader() {
            Ok(store) => store,
            Err(error) => {
                check.errors = candidates.iter().map(|c| said(c, &error.to_string())).collect();
                return check;
            }
        };
        // At most the edge's limit per request, over both kinds together, and
        // a template's id once in each. The first of the templates' go with
        // the last of the plugins'; the rest after, so none of them can take
        // a plugin's answer with it.
        let mut asks: Vec<(&[Candidate], Vec<template_blocks::Identity>)> = candidates.chunks(MAX_UPDATE_ITEMS).map(|c| (c, Vec::new())).collect();
        let room = asks.last().map_or(MAX_UPDATE_ITEMS, |(c, _)| MAX_UPDATE_ITEMS - c.len());
        let mut template_asks = template_blocks::requests(&templates, MAX_UPDATE_ITEMS).into_iter().peekable();
        if let (Some(last), Some(first)) = (asks.last_mut(), template_asks.peek())
            && first.len() <= room
        {
            last.1 = template_asks.next().unwrap_or_default();
        }
        asks.extend(template_asks.map(|templates| (&[][..], templates)));
        let mut settled: Vec<template_blocks::Identity> = Vec::new();
        let mut template_downs: Vec<template_blocks::Block> = Vec::new();
        let by_id: HashMap<&str, &Candidate> = candidates.iter().map(|c| (c.id.as_str(), c)).collect();
        let mut errors: Vec<Value> = Vec::new();
        let mut offers: BTreeMap<String, Offer> = BTreeMap::new();
        let mut blocks = Vec::new();
        // The plugins the store answered about (offline batches aren't).
        let mut answered: Vec<&Candidate> = Vec::new();
        // Those with an entry it sent that couldn't be read: whatever block
        // they have stays (it fails closed).
        let mut unread: BTreeSet<&str> = BTreeSet::new();
        // Held from the first request that carries templates until what they
        // answered is kept: the requests' own time, nothing else's. Never
        // waited on: another check asking about them now, this one asks
        // about the plugins alone (a person's, opening Settings, waits on
        // nobody's).
        let mut held = None;
        for (batch, mut asked_templates) in asks {
            if !asked_templates.is_empty() && held.is_none() {
                held = self.inner.template_checks.try_lock().ok();
            }
            if held.is_none() && !asked_templates.is_empty() {
                asked_templates.clear();
                if batch.is_empty() {
                    continue;
                }
            }
            let mut ask = UpdatesAsk {
                zeron: version::zeron().to_string(),
                plugins: batch
                    .iter()
                    .map(|c| Asked { id: c.id.clone(), version: c.version.clone(), hash: c.hash.clone(), prerelease: c.prerelease })
                    .collect(),
                templates: asked_templates.iter().map(|t| Asked { id: t.id.clone(), version: t.version.clone(), hash: t.hash.clone(), prerelease: false }).collect(),
            };
            let mut reply = client.updates(&ask, &caller).await;
            // Refused with the templates in it: the plugins are asked about
            // alone, so nothing about a template keeps them from an answer.
            if let Err(error) = &reply
                && !batch.is_empty()
                && !asked_templates.is_empty()
                && install::reason_of(error) != Some(client::reason::STORE_UNREACHABLE)
            {
                tracing::warn!(%error, "store: the templates were refused in an update check");
                ask.templates.clear();
                asked_templates.clear();
                reply = client.updates(&ask, &caller).await;
            }
            let reply = match reply {
                Ok(reply) => reply,
                // Offline: what's installed keeps running; the next check asks again.
                Err(error) => {
                    if batch.is_empty() {
                        tracing::warn!(%error, templates = asked_templates.len(), "store: the templates couldn't be checked");
                    }
                    errors.extend(batch.iter().map(|c| said(c, &error)));
                    continue;
                }
            };
            answered.extend(batch.iter());
            // Each template asked about, as the answer says: one it couldn't
            // be read about keeps the block it has.
            let anyone = reply.unreadable.iter().any(|id| id.as_deref().is_none_or(|id| !by_id.contains_key(id) && !asked_templates.iter().any(|t| t.id == id)));
            // (And one of a kind this Zeron doesn't know, whatever it is.)
            let unknown = |id: &str| reply.blocked.iter().any(|b| b.item_kind.is_none() && b.id == id);
            let readable = |t: &&template_blocks::Identity| !anyone && !unknown(&t.id) && !reply.unreadable.iter().any(|id| id.as_deref() == Some(t.id.as_str()));
            settled.extend(asked_templates.iter().filter(readable).cloned());
            for t in &asked_templates {
                if let Some(block) = reply.blocked.iter().find(|b| b.item_kind == Some(client::Kind::Template) && b.id == t.id && same_version(&b.version, &t.version)) {
                    template_downs.push(template_blocks::Block { id: t.id.clone(), version: t.version.clone(), hash: Some(t.hash.clone()), reason: plain(&block.reason, 500), at: now_secs() });
                }
            }
            for unreadable in &reply.unreadable {
                match unreadable.as_deref().and_then(|id| by_id.get(id)) {
                    Some(c) => {
                        unread.insert(c.id.as_str());
                        errors.push(said(c, &failure(client::reason::STORE_ERROR, "The store's answer about it couldn't be read")));
                    }
                    None if unreadable.as_deref().is_some_and(|id| asked_templates.iter().any(|t| t.id == id)) => {}
                    // (Whose, it doesn't say: none of this batch's blocks lifts.)
                    None => unread.extend(batch.iter().map(|c| c.id.as_str())),
                }
            }
            // One of a kind this Zeron doesn't know, naming one of these:
            // whatever block it has stays.
            for block in reply.blocked.iter().filter(|b| b.item_kind.is_none()) {
                if let Some(c) = by_id.get(block.id.as_str()) {
                    unread.insert(c.id.as_str());
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
        if held.is_some() {
            let whole = settled.len() == templates.len();
            let dir = templates_dir.clone();
            match self.changing_blocks(move || template_blocks::settle(&dir, &settled, template_downs, whole, now_secs())).await {
                Ok(whole) => {
                    check.templates_settled = whole;
                    if !whole {
                        tracing::warn!(asked = templates.len(), "store: not every template here was answered about");
                    }
                }
                Err(error) => tracing::warn!(%error, "store: what's blocked of templates couldn't be kept"),
            }
        } else if !templates.is_empty() {
            tracing::debug!("store: the templates are being checked already");
        }
        drop(held);
        // One block per plugin, for the version installed (the first the
        // store says: a hostile answer repeating one costs nothing more).
        let mut blocked: BTreeMap<&str, (&Candidate, String)> = BTreeMap::new();
        for block in &blocks {
            if block.item_kind == Some(client::Kind::Plugin)
                && let Some(c) = by_id.get(block.id.as_str())
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
                if let Err(error) = self.chosen_still(c) {
                    errors.push(said(c, &error));
                    continue;
                }
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
        check.updates = updates;
        check.errors = errors;
        check
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
            // Verified when it was found; read back as any kept update is
            // (unsigned only when its offer names no signer).
            let signed = offer.key.is_some();
            let package = tokio::task::spawn_blocking(move || package::read(&kept_path, &install::WORDS).and_then(|raw| install::parse_from_store(raw, signed)))
                .await
                .map_err(|_| "Couldn't read it".to_string())??;
            let (publisher, publisher_id) = super::vouched(&publisher)?;
            if package.hash != offer.hash || offer.key.as_deref().is_some_and(|k| Some(k) != package.key.as_deref()) {
                return Err(failure(client::reason::STORE_MISMATCH, "The update kept isn't the store's"));
            }
            let store = install::StoreSource { id: c.id.clone(), publisher, publisher_id, hash: package.hash.clone(), key: package.key.clone(), last_key: None, verified: offer.verified, verified_at: None, verified_alternative: None, instead_of_unverified: false, replaced: None };
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
            // (Unsigned, a deploy token's: no key to have changed.)
            new_key: c.key.as_deref().zip(package.key.as_deref()).is_some_and(|(was, now)| was != now),
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
            held: false,
        };
        let keep = found.refused.is_none().then_some(&package);
        let _finding = install::finding();
        self.chosen_still(c)?;
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

/// The store's page for `id`: its newest listed version, its newest
/// verified one, who owns it.
async fn store_detail(client: &StoreClient, caller: &Caller, id: &str) -> Result<client::Detail, RpcError> {
    client.plugin(id, caller).await.map(|(_, detail)| detail).map_err(failed)
}

/// The newest verified version of template `id` this Zeron can run, other
/// than `asked`: as its pages say, and its own record too. None when there's
/// none, the store can't say, or it can't go over what's here (`place`): it's
/// what's installed, or older than a verified one. (Never one older than it.)
/// A pre-release only where this device takes them of it (`prereleases`).
async fn verified_template(client: &StoreClient, caller: &Caller, id: &str, asked: &str, publisher: &client::Publisher, place: Option<&template_packages::Place>, prereleases: bool) -> Option<install::Alternative> {
    super::vouched(publisher).ok()?;
    let zeron = version::zeron();
    let mut cursor: Option<String> = None;
    for _ in 0..ALTERNATIVE_PAGES {
        let page = client.versions(id, cursor.as_deref(), Some(50), caller).await.ok()?;
        let found = page["versions"].as_array().into_iter().flatten().find(|v| {
            v["verified"] == true
                && v["status"] == "listed"
                && v["yanked"] != true
                && v["version"].as_str().is_some_and(|x| !same_version(x, asked) && Version::parse(x).is_ok())
                && v["version"].as_str().is_some_and(|x| prereleases || Version::parse(x).is_ok_and(|x| !x.is_prerelease()))
                && version::accepts(v["engines"]["zeron"].as_str(), &zeron).is_ok()
        });
        if let Some(found) = found {
            let version = found["version"].as_str()?.to_owned();
            if place.is_some_and(|place| !install::newer_by_precedence(&version, &place.version) && !place.takes_instead(&version)) {
                return None;
            }
            let (_, record) = client.version(id, &version, caller).await.ok()?;
            return (record.verified && record.status == "listed" && !record.yanked).then(|| install::Alternative {
                version,
                added: Vec::new(),
                removed: Vec::new(),
                notes: notes(found["notes"].as_str().unwrap_or_default(), listing::MAX_NOTES_BYTES),
                verified_at: record.verified_at,
            });
        }
        cursor = Some(page["next_cursor"].as_str()?.to_owned());
    }
    None
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

/// `{ok: false, error: "not_owner", ids}`: a token's scope names ids that
/// aren't the account's in the store.
fn not_owner(ids: &[&str]) -> Value {
    let mut value = not_ok(&failure(
        "not_owner",
        format!("{} isn't yours in the store: a token publishes what you already have there (a first version comes from a signed-in device)", ids.join(", ")),
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
