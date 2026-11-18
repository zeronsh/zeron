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

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use zeron_rpc::{RpcError, RpcReply, methods, parse_params};

use super::client::{self, Caller, Installed as Asked, Offer, PackageRecord, Search, StoreClient, UpdatesAsk};
use super::deploy_key::DeployKey;
use super::publish;
use crate::plugins::install::{self, Found, STORE, Source, failure};
use crate::plugins::listing::{self, CATEGORIES};
use crate::plugins::status::Blocked;
use crate::plugins::version::{self, Version};
use crate::plugins::{Plugins, capability, failed, lock, manifest, now_secs, package, reviews, signing};

/// The most one store file fetched for showing may be.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Versions of a plugin whose files stay cached.
const CACHED_VERSIONS: usize = 3;
/// The most every store file cached here comes to: past it, the oldest
/// versions' go.
const MAX_CACHE_BYTES: u64 = 256 * 1024 * 1024;
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
            | methods::STORE_CATEGORIES
            | methods::STORE_FILE
            | methods::SET_PLUGIN_PRERELEASES
            | methods::PREPARE_PLUGIN_PUBLISH
            | methods::PUBLISH_PLUGIN
            | methods::STORE_MY_PLUGINS
            | methods::STORE_VERSION_ACTION
            | methods::LIST_DEPLOY_KEYS
            | methods::CREATE_DEPLOY_KEY
            | methods::REVOKE_DEPLOY_KEY
            | methods::STORE_REPORT
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
    /// Every version the caller may see, not only the latest few.
    #[serde(default)]
    versions: bool,
    /// Judged for this version of Zeron (another device's), not this one's.
    #[serde(default)]
    for_zeron: Option<String>,
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
    id: String,
    name: String,
    /// Epoch ms.
    #[serde(default)]
    expires: Option<i64>,
}

#[derive(Deserialize)]
struct RevokeKeyParams {
    id: String,
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
}

/// `{ok: false, error, message}` from a failure's `"<word>: <words>"`.
fn not_ok(error: &str) -> Value {
    let word = install::reason_of(error).unwrap_or("failed");
    let message = error.strip_prefix(&format!("{word}: ")).unwrap_or(error);
    json!({ "ok": false, "error": word, "message": message })
}

/// Characters that change how the text around them reads (direction
/// overrides, invisible joiners): never kept from the store's words.
fn invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' | '\u{061C}')
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
                if p.versions {
                    let (versions, _) = client.versions(&p.id, &caller).await.map_err(failed)?;
                    reply["versions"] = versions.get("versions").cloned().unwrap_or(json!([]));
                }
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
                let prepared = tokio::task::spawn_blocking(move || publish::read(&dir))
                    .await
                    .map_err(|_| failed("Couldn't read it"))?
                    .map_err(failed)?;
                let mut reply = prepared.shown();
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
                RpcReply::value(&client.my_plugins(&caller).await.map_err(failed)?)
            }
            methods::STORE_VERSION_ACTION => {
                let p: VersionActionParams = parse_params(params)?;
                manifest::check_id(&p.id).map_err(failed)?;
                Version::parse(&p.version).map_err(failed)?;
                if !matches!(p.action.as_str(), "withdraw" | "yank" | "unyank") {
                    return Err(failed("The action is withdraw, yank or unyank"));
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
                let p: IdParams = parse_params(params)?;
                manifest::check_id(&p.id).map_err(failed)?;
                let (client, caller) = self.store_as_person().await.map_err(failed)?;
                RpcReply::value(&client.keys(&p.id, &caller).await.map_err(failed)?)
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
            methods::REVOKE_DEPLOY_KEY => {
                let p: RevokeKeyParams = parse_params(params)?;
                manifest::check_id(&p.id).map_err(failed)?;
                let (client, caller) = self.store_as_person().await.map_err(failed)?;
                client.revoke_key(&p.id, p.key.trim(), &caller).await.map_err(failed)?;
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
        let packed = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
            // Re-signed as it is now, then read as it'll be sent.
            this.auto_resign(&id_owned);
            let prepared = publish::read(&dir)?;
            if prepared.hash != hash {
                return Err(failure("changed", "It changed since it was checked: check it again"));
            }
            if !prepared.ok() {
                return Err(failure(prepared.problems[0].rule, publish::lines(&prepared.problems).join("; ")));
            }
            let key = signing::load_or_create_key(&this.inner.data_dir)?;
            prepared.pack(&key)
        })
        .await;
        let raw = match packed {
            Ok(Ok(raw)) => raw,
            Ok(Err(error)) => return not_ok(&error),
            Err(_) => return not_ok("Couldn't pack it"),
        };
        match client.publish(id, raw, &caller).await {
            Ok(mut reply) => {
                reply["ok"] = json!(true);
                reply
            }
            Err(error) => not_ok(&error),
        }
    }

    /// A deploy key for `id`, made here and registered at the edge with a
    /// fresh sign-in's ticket and its proof: its secret, shown this once.
    async fn create_deploy_key(&self, p: CreateKeyParams) -> Value {
        if let Err(error) = manifest::check_id(&p.id) {
            return not_ok(&failure("invalid", error));
        }
        let name = plain(&p.name, 64);
        if name.is_empty() {
            return not_ok(&failure("invalid", "Give the key a name (where it's used, like \"GitHub Actions\")"));
        }
        let (client, caller) = match self.store_as_person().await {
            Ok(store) => store,
            Err(error) => return not_ok(&error),
        };
        // A deploy key is for a plugin you have in the store already (its
        // first version comes from your machine; amendment 17).
        match client.my_plugins(&caller).await {
            Ok(mine) if mine["plugins"].as_array().into_iter().flatten().any(|plugin| plugin["id"] == p.id.as_str()) => {}
            Ok(_) => return not_published(&p.id),
            Err(error) => return not_ok(&error),
        }
        let step_up = lock(&self.inner.step_up).clone().and_then(|f| f());
        let Some(step_up) = step_up else {
            return not_ok(&failure("step_up_required", "Making a deploy key needs a fresh sign-in"));
        };
        let key = match DeployKey::generate() {
            Ok(key) => key,
            Err(error) => return not_ok(&error),
        };
        let mut body = json!({ "key": key.public(), "name": name, "proof": key.proof(&p.id), "stepUp": step_up });
        if let Some(expires) = p.expires {
            body["expires"] = json!(expires);
        }
        match client.add_key(&p.id, body, &caller).await {
            Ok(registered) => {
                tracing::info!(plugin = %p.id, key = %key.public(), "store: a deploy key was made");
                json!({ "ok": true, "key": registered, "secret": key.secret() })
            }
            // (The edge's word for an id this account doesn't own.)
            Err(error) if install::reason_of(&error) == Some("not_found") => not_published(&p.id),
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
        let id = id.trim();
        let (package, source) = self.fetch_from_store(id, version, reinstalling).await?;
        let reply = self.install_package(package, source).await?;
        // Counted once per account (the edge's to say): a new install here.
        if let RpcReply::Value(value) = &reply
            && value["update"] == false
            && let Ok((client, caller)) = self.store_reader()
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

    /// `id`'s latest version in the store for this caller (or `version`),
    /// fetched, verified and checked against the store's record: the
    /// package and its source. `installed`: it's the version installed here
    /// (a reinstall), which may be yanked since.
    pub(in crate::plugins) async fn fetch_from_store(&self, id: &str, version: Option<&str>, installed: bool) -> Result<(install::Package, Source), RpcError> {
        manifest::check_id(id).map_err(failed)?;
        let (client, caller) = self.store_reader()?;
        let wanted = version.map(str::trim).filter(|v| !v.is_empty());
        let not_listed = |error: &str| install::reason_of(error) == Some(client::reason::NOT_IN_STORE);
        // What the store lists, to everyone.
        let listed = match client.plugin(id, &caller).await {
            Ok((_, detail)) => match wanted {
                None => (!detail.version.is_empty()).then(|| (detail.version.clone(), detail.package.clone(), detail.author.clone())),
                Some(wanted) => {
                    let (_, versions) = client.versions(id, &caller).await.map_err(failed)?;
                    let found = versions.into_iter().find(|v| v.version == wanted);
                    match found {
                        Some(found) => {
                            let usable = matches!(found.status.as_str(), "approved" | "pending") || (installed && found.status == "yanked");
                            if (found.yanked && !installed) || !usable {
                                return Err(failed(failure(client::reason::NOT_IN_STORE, format!("Version {wanted} of {id} can't be installed any more"))));
                            }
                            let record = PackageRecord { hash: found.hash, sha256: found.sha256, bytes: found.bytes, key: found.key };
                            Some((found.version, record, detail.author.clone()))
                        }
                        None => None,
                    }
                }
            },
            Err(error) if not_listed(&error) => None,
            Err(error) => return Err(failed(error)),
        };
        let (version, record, publisher) = match listed {
            Some(listed) => listed,
            // Not listed to everyone (yet): your own, in review, as your
            // plugins in the store say it, for your devices alone.
            None => self.own_record(&client, id, wanted, installed).await.map_err(failed)?,
        };
        let raw = client.package(id, &version, &caller).await.map_err(failed)?;
        let (id_owned, version_owned) = (id.to_owned(), version.clone());
        let (package, store) = tokio::task::spawn_blocking(move || super::verify(raw, &id_owned, &version_owned, &record, &publisher))
            .await
            .map_err(|_| failed("Couldn't read it"))?
            .map_err(failed)?;
        let source = Source { kind: STORE.into(), url: None, path: None, git_ref: None, commit: None, version: String::new(), installed_at: 0, store: Some(store) };
        Ok((package, source))
    }

    /// Plugin `id`'s version `wanted` (none: its latest) among your own
    /// plugins in the store (`/me/plugins`): its version, its record, and
    /// you as its publisher. `not_in_store` when it isn't yours, or there's
    /// no such version to install.
    async fn own_record(&self, client: &StoreClient, id: &str, wanted: Option<&str>, installed: bool) -> Result<(String, PackageRecord, client::Publisher), String> {
        let missing = || failure(client::reason::NOT_IN_STORE, format!("The store has no {id}{} for you", wanted.map(|w| format!(" {w}")).unwrap_or_default()));
        let Ok((_, caller)) = self.store_as_person().await else {
            return Err(missing());
        };
        let mine = client.my_plugins(&caller).await?;
        let Some(plugin) = mine["plugins"].as_array().into_iter().flatten().find(|p| p["id"] == id) else {
            return Err(missing());
        };
        let versions: Vec<client::VersionRecord> = plugin["versions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect();
        let usable = |v: &client::VersionRecord| matches!(v.status.as_str(), "approved" | "pending") || (installed && v.status == "yanked");
        let chosen = match wanted {
            Some(wanted) => versions.into_iter().find(|v| v.version == wanted && usable(v)),
            None => versions
                .into_iter()
                .filter(|v| usable(v) && !v.yanked)
                .filter_map(|v| Version::parse(&v.version).ok().map(|at| (at, v)))
                .max_by(|a, b| a.0.cmp(&b.0))
                .map(|(_, v)| v),
        };
        let found = chosen.ok_or_else(missing)?;
        let publisher: client::Publisher = serde_json::from_value(plugin["author"].clone()).unwrap_or_default();
        let record = PackageRecord { hash: found.hash, sha256: found.sha256, bytes: found.bytes, key: found.key };
        Ok((found.version, record, publisher))
    }

    /// The batched check for every plugin installed from the store (or
    /// `only`): updates kept as any source's are (`install::Found`), blocked
    /// versions turned off. `(updates, errors)`, as `check_updates` says them.
    pub(in crate::plugins) async fn check_store_updates(&self, only: Option<&str>) -> (Vec<Value>, Vec<Value>) {
        let decisions = lock(&self.inner.state).decisions.clone();
        let prerelease = lock(&self.inner.state).prerelease.clone();
        let candidates: Vec<Candidate> = lock(&self.inner.entries)
            .iter()
            .filter(|e| !e.item.builtin && !e.item.own && only.is_none_or(|id| id == e.item.id))
            .filter_map(|e| {
                let store = e.store.as_ref()?;
                let allowed = decisions.get(&e.item.id).filter(|d| d.allowed).map_or_else(|| e.record.clone(), |d| d.allowed_record());
                Some(Candidate {
                    id: e.item.id.clone(),
                    name: e.item.name.clone(),
                    version: e.item.version.clone(),
                    hash: e.item.hash.clone(),
                    publisher_id: store.publisher_id.clone(),
                    permissions: e.item.permissions.clone(),
                    allowed,
                    prerelease: prerelease.contains(&e.item.id),
                })
            })
            .collect();
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
            for unreadable in reply.unreadable.iter().flatten() {
                if let Some(c) = by_id.get(unreadable.as_str()) {
                    errors.push(said(c, &failure(client::reason::STORE_ERROR, "The store's answer about it couldn't be read")));
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
        for block in &blocks {
            if let Some(c) = by_id.get(block.id.as_str())
                && same_version(&block.version, &c.version)
            {
                self.block(c, &plain(&block.reason, 500)).await;
            }
        }
        let plugins_dir = self.inner.plugins_dir.clone();
        let mut updates = Vec::new();
        for c in &candidates {
            let Some(offer) = offers.remove(&c.id) else {
                install::forget_found(&plugins_dir, &c.id);
                continue;
            };
            match self.store_found(&client, &caller, c, &offer).await {
                Ok(found) => {
                    let mut shown = found.shown();
                    shown["id"] = json!(c.id);
                    shown["name"] = json!(c.name);
                    updates.push(shown);
                }
                Err(error) => errors.push(said(c, &error)),
            }
        }
        (updates, errors)
    }

    /// `offer` for `c`, fetched and verified (or the copy already kept, when
    /// it's the same version), compared with what's installed, and kept.
    async fn store_found(&self, client: &StoreClient, caller: &Caller, c: &Candidate, offer: &Offer) -> Result<Found, String> {
        let plugins_dir = self.inner.plugins_dir.clone();
        let record = PackageRecord { hash: offer.hash.clone(), sha256: offer.sha256.clone(), bytes: offer.bytes, key: offer.key.clone() };
        let kept_path = install::found_package_path(&plugins_dir, &c.id);
        let kept = install::found(&plugins_dir, &c.id, &c.hash)
            .filter(|f| f.refused.is_none() && f.hash == offer.hash && f.version == offer.version && kept_path.is_file());
        let (id, version, publisher) = (c.id.clone(), offer.version.clone(), offer.publisher());
        let (package, store) = if kept.is_some() {
            // Verified when it was found; read back as any kept update is.
            let package = tokio::task::spawn_blocking(move || package::read(&kept_path, &install::WORDS).and_then(install::parse))
                .await
                .map_err(|_| "Couldn't read it".to_string())??;
            let (publisher, publisher_id) = super::vouched(&publisher)?;
            if package.hash != offer.hash || offer.key.as_deref().is_some_and(|k| k != package.key) {
                return Err(failure(client::reason::STORE_MISMATCH, "The update kept isn't the store's"));
            }
            let store = install::StoreSource { id: c.id.clone(), publisher, publisher_id, hash: package.hash.clone(), key: Some(package.key.clone()) };
            (package, store)
        } else {
            let raw = client.package(&c.id, &offer.version, caller).await?;
            tokio::task::spawn_blocking(move || super::verify(raw, &id, &version, &record, &publisher))
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
            new_key: false,
            new_owner,
            in_review: offer.in_review,
            notes: notes(&offer.notes, listing::MAX_NOTES_BYTES),
        };
        let keep = found.refused.is_none().then_some(&package);
        install::keep_found(&plugins_dir, &c.id, &found, keep)?;
        Ok(found)
    }

    /// `c`'s installed version is blocked by the store: turned off, the
    /// reason kept, and you're told. Not again once you turned it back on.
    async fn block(&self, c: &Candidate, reason: &str) {
        // Still the version the store blocked (not one installed while the
        // check was out).
        self.rescan().await;
        if self.entry(&c.id).is_none_or(|e| e.item.version != c.version || e.item.hash != c.hash) {
            return;
        }
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
fn not_published(id: &str) -> Value {
    not_ok(&failure(
        "not_published",
        format!("{id} isn't in the store as yours yet: publish its first version from this machine (`zeron plugin publish {id}`), then make a deploy key for it"),
    ))
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
        assert_eq!(media_type("shots/A.PNG"), "image/png");
    }
}
