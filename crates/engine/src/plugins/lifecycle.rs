//! `InstallPlugin`, `CheckPluginUpdates`, `UpdatePlugin`, `ExportPlugin`:
//! the IO around `install.rs`. Uninstalling is `RemovePlugin` (mod.rs): its
//! folder, its decision, what it stored, and the update kept for it.
//!
//! Consent stays the app's. Installing records a decision only for a plugin
//! signed by this device's own key, and even then only for what runs inside
//! the plugin's sandbox (`capability::inside_sandbox`): native, commands,
//! service, devices, ui.replace, the agent controls (accounts.switch,
//! models.set) and messages.send are always asked. Anyone else's (another of your
//! devices' included) is listed `pending`, and the app's consent window
//! asks. An update signed by the same key keeps what you allowed; one that
//! grows its capability record waits for you again (`status::decide`). An
//! Unknown author's update always waits: its decision was for the content.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use zeron_rpc::{RpcError, RpcReply, methods, parse_params};

use super::capability;
use super::install::{self, Package, Source};
use super::package;
use super::template_packages;
use super::status::{Author, Decision, Status};
use super::{Plugins, failed, lock, now_secs};

/// The first check after the engine starts, then how often.
pub(super) const FIRST_CHECK: Duration = Duration::from_secs(90);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// How often the store's take-down feed is read (a take-down reaches a
/// device within this and the feed's 60 s of cache).
const TAKEDOWN_EVERY: Duration = Duration::from_secs(15 * 60);

/// [`TAKEDOWN_EVERY`] (and the wait before the first read): or, in a debug
/// build only, `ZERON_TAKEDOWN_EVERY_SECS` (a local end-to-end run, which
/// can't wait 15 minutes). A release build reads no such setting.
pub(super) fn takedown_every() -> (Duration, Duration) {
    takedown_every_from(local_every())
}

/// `ZERON_TAKEDOWN_EVERY_SECS`, in a debug build only, when it shortens.
pub(super) fn local_every() -> Option<Duration> {
    let setting = cfg!(debug_assertions).then(|| std::env::var("ZERON_TAKEDOWN_EVERY_SECS").ok()).flatten();
    shorter(setting.as_deref())
}

/// A setting of `secs`, taken only when it shortens: 1 s up to the built-in
/// interval. Zero, junk, or anything longer is ignored, so nothing makes a
/// device read the feed (or look over its account's publishes) less often.
fn shorter(secs: Option<&str>) -> Option<Duration> {
    secs.and_then(|s| s.trim().parse::<u64>().ok()).filter(|s| (1..=TAKEDOWN_EVERY.as_secs()).contains(s)).map(Duration::from_secs)
}

/// The first read's wait and the interval, for a setting: the first read
/// never waits longer than it would anyway.
fn takedown_every_from(every: Option<Duration>) -> (Duration, Duration) {
    every.map_or((FIRST_CHECK, TAKEDOWN_EVERY), |every| (every.min(FIRST_CHECK), every))
}

#[derive(Deserialize)]
struct VerifiedInsteadParams {
    id: String,
    hash: String,
}

#[derive(Deserialize)]
struct InstallParams {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default, rename = "ref")]
    git_ref: Option<String>,
    #[serde(default)]
    folder: Option<String>,
    /// An installed plugin's id: it again, fetched fresh from where it came
    /// from, in place of what's there (a tampered one's way back).
    #[serde(default)]
    reinstall: Option<String>,
    /// A file sent from another of your devices (`StagePluginPackage`), and
    /// its name there (what its source says).
    #[serde(default)]
    upload: Option<String>,
    #[serde(default)]
    name: Option<String>,
    /// A store id: its latest version for you (`version`: that one).
    #[serde(default)]
    store: Option<String>,
    #[serde(default)]
    version: Option<String>,
}

#[derive(Deserialize)]
struct StageParams {
    #[serde(default)]
    upload: Option<String>,
    data: String,
}

#[derive(Deserialize)]
struct CheckParams {
    #[serde(default)]
    id: Option<String>,
}

#[derive(Deserialize)]
struct UpdateParams {
    id: String,
    /// One of many ("update all"): a store update Zeron hasn't verified is
    /// refused (`not_verified`), never taken this way.
    #[serde(default)]
    verified_only: bool,
}

#[derive(Deserialize)]
struct ExportParams {
    id: String,
    #[serde(default)]
    to: Option<String>,
}

pub fn handles(method: &str) -> bool {
    matches!(
        method,
        methods::INSTALL_PLUGIN
            | methods::STAGE_PLUGIN_PACKAGE
            | methods::CHECK_PLUGIN_UPDATES
            | methods::UPDATE_PLUGIN
            | methods::INSTALL_VERIFIED_PLUGIN
            | methods::EXPORT_PLUGIN
    )
}

/// Who may send a plugin file here: this device's app, or another of your
/// devices as the edge verified the link (an agent installs by `path`).
fn uploads_origin() -> Result<super::reviews::Origin, RpcError> {
    super::reviews::Origin::of(&zeron_rpc::caller())
        .ok_or_else(|| failed("Only Zeron on one of your devices can send a plugin file here"))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| failed("It stopped part way"))?
        .map_err(failed)
}

impl Plugins {
    pub(super) async fn lifecycle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
        match method {
            methods::INSTALL_PLUGIN => {
                let p: InstallParams = parse_params(params)?;
                if let Some(id) = p.reinstall.filter(|s| !s.trim().is_empty()) {
                    return self.reinstall(id.trim()).await;
                }
                let path = p.path.filter(|s| !s.trim().is_empty());
                let url = p.url.filter(|s| !s.trim().is_empty());
                if let Some(id) = p.store.filter(|s| !s.trim().is_empty()) {
                    if path.is_some() || url.is_some() || p.upload.as_deref().is_some_and(|u| !u.trim().is_empty()) {
                        return Err(failed("Give one of a store id (`store`), a package file (`path`), a link (`url`) or a file sent here (`upload`)"));
                    }
                    return self.install_from_store(id.trim(), p.version.as_deref(), false).await;
                }
                if let Some(upload) = p.upload.filter(|s| !s.trim().is_empty()) {
                    if path.is_some() || url.is_some() {
                        return Err(failed("Give a package file (`path`), a link (`url`) or a file sent here (`upload`)"));
                    }
                    let origin = uploads_origin()?;
                    let raw = lock(&self.inner.uploads).take(&origin, upload.trim())?;
                    let package = blocking(move || install::parse(raw)).await?;
                    let name = p.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(|n| {
                        // Only the file's name: its folder is the other device's.
                        n.rsplit(['/', '\\']).next().unwrap_or(n).chars().take(200).collect::<String>()
                    });
                    let source = Source {
                        kind: "file".into(),
                        url: None,
                        path: name,
                        git_ref: None,
                        commit: None,
                        version: String::new(),
                        installed_at: 0,
                        store: None,
                    };
                    return self.install_package(package, source).await;
                }
                let (package, source) = match (path, url) {
                    (Some(path), None) => {
                        let path = PathBuf::from(path.trim());
                        if !path.is_absolute() {
                            return Err(failed("Give the package file's full path"));
                        }
                        let read = path.clone();
                        let package = blocking(move || package::read(&read, &install::WORDS).and_then(install::parse)).await?;
                        let shown = std::fs::canonicalize(&path).unwrap_or(path);
                        let source = Source {
                            kind: "file".into(),
                            url: None,
                            path: Some(shown.display().to_string()),
                            git_ref: None,
                            commit: None,
                            version: String::new(),
                            installed_at: 0,
                            store: None,
                        };
                        (package, source)
                    }
                    (None, Some(url)) => {
                        let origin = install::origin(&url, p.git_ref.as_deref(), p.folder.as_deref()).map_err(failed)?;
                        install::fetch(&origin, &self.scratch()?).await.map_err(failed)?
                    }
                    _ => return Err(failed("Give a package file (`path`) or a link (`url`)")),
                };
                self.install_package(package, source).await
            }
            methods::STAGE_PLUGIN_PACKAGE => {
                let p: StageParams = parse_params(params)?;
                let origin = uploads_origin()?;
                let (upload, bytes) = lock(&self.inner.uploads).stage(origin, p.upload.as_deref(), &p.data)?;
                RpcReply::value(&json!({ "upload": upload, "bytes": bytes }))
            }
            methods::CHECK_PLUGIN_UPDATES => {
                let p: CheckParams = parse_params(params)?;
                RpcReply::value(&self.check_updates(p.id.as_deref()).await)
            }
            methods::UPDATE_PLUGIN => {
                let p: UpdateParams = parse_params(params)?;
                self.update(&p.id, p.verified_only).await
            }
            methods::INSTALL_VERIFIED_PLUGIN => {
                let p: VerifiedInsteadParams = parse_params(params)?;
                self.install_verified_alternative(p.id.trim(), &p.hash).await
            }
            methods::EXPORT_PLUGIN => {
                let p: ExportParams = parse_params(params)?;
                // R29: an agent exports to Zeron's plugin-exports folder
                // only; a path of its choosing is the person's (`zeron
                // plugin export --to`, their save dialog).
                if !zeron_rpc::caller().is_local() && p.to.as_deref().is_some_and(|to| !to.trim().is_empty()) {
                    return Err(failed(
                        "Agents export to Zeron's plugin-exports folder: leave out `to` (the reply says where it went)",
                    ));
                }
                self.rescan().await;
                let entry = self.entry(&p.id).ok_or_else(|| failed(format!("There's no plugin {}", p.id)))?;
                let name = format!(
                    "{}-{}.{}",
                    entry.item.id,
                    if entry.item.version.is_empty() { "1" } else { entry.item.version.as_str() },
                    install::EXTENSION
                );
                let out = match p.to.filter(|s| !s.trim().is_empty()).map(|s| PathBuf::from(s.trim())) {
                    Some(to) if !to.is_absolute() => return Err(failed("Give a full path to save it to")),
                    Some(to) if to.extension().is_some_and(|e| e == install::EXTENSION) => to,
                    Some(to) => to.join(name),
                    None => self.inner.data_dir.join("plugin-exports").join(name),
                };
                if out.starts_with(&self.inner.plugins_dir) {
                    return Err(failed("Save it outside the plugins folder"));
                }
                let this = self.clone();
                let id = p.id.clone();
                let dir = PathBuf::from(&entry.item.dir);
                let own = entry.item.own;
                let written = out.clone();
                let bytes = blocking(move || {
                    if own {
                        this.auto_resign(&id);
                    }
                    let raw = install::pack_folder(&dir)?;
                    if let Some(parent) = written.parent() {
                        std::fs::create_dir_all(parent).map_err(|_| "Couldn't make that folder".to_string())?;
                    }
                    std::fs::write(&written, &raw).map_err(|_| "Couldn't save it there".to_string())?;
                    Ok(raw.len())
                })
                .await?;
                RpcReply::value(&json!({ "path": out.display().to_string(), "bytes": bytes }))
            }
            _ => Err(RpcError::UnknownMethod(method.to_owned())),
        }
    }

    /// Where fetched repositories are put together (inside the plugins
    /// folder's dot entries, so never scanned or watched as a plugin).
    fn scratch(&self) -> Result<PathBuf, RpcError> {
        let dir = self.inner.plugins_dir.join(install::INCOMING_DIR);
        std::fs::create_dir_all(&dir).map_err(|_| failed("Couldn't make room for it"))?;
        Ok(dir)
    }

    /// Whether the call being served is the person's: this device's app, or
    /// another of their devices as the edge verified the link. An agent's
    /// (or an unverified dial's) isn't.
    pub(in crate::plugins) fn by_person() -> bool {
        super::reviews::Origin::of(&zeron_rpc::caller()).is_some()
    }

    /// The refusal when an agent would replace installed store plugin
    /// `name`'s code: an update is the person's click.
    pub(in crate::plugins) fn needs_person(name: &str) -> RpcError {
        failed(install::failure(
            install::reason::NEEDS_PERSON,
            format!("{name} is installed already: only the person updates it, in Zeron's window (check_plugin_updates tells them there's an update)"),
        ))
    }

    /// Install a verified package, keep the decisions right, and say what
    /// changed.
    pub(in crate::plugins) async fn install_package(&self, package: Package, source: Source) -> Result<RpcReply, RpcError> {
        self.install_package_keeping(package, source, false).await
    }

    /// `install_package`; `keep_storage`: what it stored stays even though
    /// its folder is new here (it's the same plugin, installed again).
    async fn install_package_keeping(&self, package: Package, mut source: Source, keep_storage: bool) -> Result<RpcReply, RpcError> {
        self.rescan().await;
        let id = package.id().to_owned();
        // Made for another Zeron: never installed here, from anywhere.
        install::compatible(&package.manifest).map_err(failed)?;
        // What a decision about it is bound to: its owner in the store, or
        // the key that signed it.
        // (Only the store's can be unsigned: anything else was refused.)
        let signer = package.key.clone().unwrap_or_default();
        let binding = source.store().map_or_else(|| signer.clone(), |s| s.binding());
        let mut old_binding = None;
        // Signed by another key than the installed one: only its author's
        // own (a new machine), which asks you again; or, from the store,
        // whoever owns its id there (store to store, the store's record
        // decides; into the store from elsewhere, the same username).
        let mut rotated_from = None;
        let person = Self::by_person();
        if let Some(entry) = self.entry(&id) {
            if entry.item.builtin {
                return Err(failed(super::manifest::RESERVED_ID_ERROR));
            }
            if entry.item.own {
                return Err(failed(format!(
                    "You're making a plugin called {id} on this device. Remove or rename it first to install this one."
                )));
            }
            // From the store over one installed, or anything over one from
            // the store (a file or a link would also take it out of the
            // store's take-downs): new code under what the person allowed,
            // so theirs to do (as `UpdatePlugin` is).
            if (source.store().is_some() || entry.store.is_some()) && !person {
                return Err(Self::needs_person(&entry.item.name));
            }
            old_binding = entry.binding().map(str::to_owned);
            // From the store over a store version: which one, and whether
            // it was verified (the consent window says so).
            if let (Some(before), Some(store)) = (&entry.store, source.store.as_mut().filter(|_| source.kind == install::STORE)) {
                store.replaced = Some(install::ReplacedVersion { version: entry.item.version.clone(), verified: before.verified });
                // Unsigned (a deploy token's): the key the version before
                // was signed with carries on, so a later signed version
                // signed by another still says its key changed.
                if store.key.is_none() {
                    store.last_key = entry.key().map(str::to_owned).or_else(|| before.last_key.clone());
                }
            }
            if let Some(key) = entry.key().filter(|k| Some(*k) != package.key.as_deref()) {
                let takes_over = match (&entry.store, source.store()) {
                    (Some(_), Some(_)) => true,
                    (None, Some(store)) => matches!(
                        &entry.item.author,
                        Author::Verified { username: Some(name), .. } if name.trim().eq_ignore_ascii_case(store.publisher.trim())
                    ),
                    _ => self.same_account(key, &signer, &id).await,
                };
                if takes_over {
                    rotated_from = Some(key.to_owned());
                }
            }
        }
        source.version = package.manifest.version.clone();
        source.installed_at = now_secs();
        let local = zeron_rpc::caller().is_local();
        let author = match source.store() {
            Some(store) => super::scan::store_author(store),
            None => self.resolve_now(&signer, Some(&id)).await,
        };
        let from_store = source.store().is_some();
        // A store version Zeron hasn't verified (by the record fetched for
        // this install): taking it always asks you, even when it asks for
        // nothing new (amendment 45).
        let unverified = source.store().is_some_and(|s| !s.verified);
        let version = package.manifest.version.clone();
        let plugins_dir = self.inner.plugins_dir.clone();
        let staged = package.clone();
        // Held while the folder is swapped: a scan in between would see it
        // gone and forget its decision.
        let scan_guard = self.inner.scan_lock.lock().await;
        let replaced = blocking(move || install::install(&plugins_dir, &staged, &source, rotated_from.as_deref())).await?;
        let record = capability::record(&package.manifest);
        if replaced.is_none() && !keep_storage {
            // New here: nothing a plugin once under this id kept is its.
            super::forget_storage(&self.inner.plugins_dir, &id);
        }
        self.update_state(|state| match &replaced {
            None => {
                state.decisions.remove(&id);
                // Signed on this device: what runs inside its sandbox is
                // allowed; anything outside it waits for you.
                // (Only when this device's own app installs it: an agent's
                // or another device's install always waits.)
                if author == Author::You && local {
                    let granted = capability::inside_sandbox(&record);
                    let permissions = granted["permissions"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|p| p.as_str().map(str::to_owned))
                        .collect();
                    state.decisions.insert(
                        id.clone(),
                        Decision {
                            allowed: true,
                            author_key: package.key.clone(),
                            permissions,
                            content_hash: None,
                            off: false,
                            grants: Some(granted),
                            reask: false,
                        },
                    );
                }
            }
            // Signed by its author's new key, or another owner's in the
            // store (or into the store from elsewhere): what you allowed
            // stands against that now, and asks you again.
            Some(_) if old_binding.as_deref() != Some(binding.as_str()) => {
                if let Some(d) = state.decisions.get_mut(&id)
                    && d.allowed
                {
                    d.author_key = Some(binding.clone());
                    d.content_hash = None;
                    d.reask = true;
                }
            }
            Some(old) => {
                // Allowed by its content (an author not known): the update
                // is signed by the same key, but nobody knows whose that is,
                // so it waits for you again.
                if let Some(d) = state.decisions.get_mut(&id)
                    && d.allowed
                    && d.author_key.is_none()
                    && d.content_hash.as_deref() == Some(old.hash.as_str())
                    && old.hash != package.hash
                {
                    d.content_hash = Some(package.hash.clone());
                    d.reask = true;
                }
            }
        })?;
        if replaced.is_some() && unverified {
            self.update_state(|state| {
                if let Some(d) = state.decisions.get_mut(&id)
                    && d.allowed
                {
                    d.reask = true;
                }
            })?;
        }
        self.update_state(|state| {
            // A block was for the version it replaced.
            if state.blocked.get(&id).is_some_and(|b| b.version != version) {
                state.blocked.remove(&id);
            }
            // Installing a pre-release from the store takes its later ones
            // (the person's choice: an agent's install never opts them in).
            if from_store && person && super::version::Version::parse(&version).is_ok_and(|v| v.is_prerelease()) {
                state.prerelease.insert(id.clone());
            }
        })?;
        drop(scan_guard);
        install::forget_found(&self.inner.plugins_dir, &id);
        self.rescan().await;
        let entry = self.entry(&id).ok_or_else(|| failed(format!("There's no plugin {id}")))?;
        let change = install::grant_change(
            replaced.as_ref().map(|r| r.permissions.as_slice()).unwrap_or_default(),
            &package.manifest.permissions,
        );
        let mut reply = json!({
            "plugin": entry.item,
            "update": replaced.is_some(),
            "added": change.added,
            "removed": change.removed,
            "waiting": matches!(entry.item.status, Status::Pending | Status::Tampered),
        });
        if let Some(diff) = &entry.item.digest_diff {
            reply["digest_diff"] = diff.clone();
        }
        if let Some(old) = &replaced {
            reply["from"] = json!(old.version);
        }
        RpcReply::value(&reply)
    }

    /// Check every installed plugin's source (or `only`'s) for a new version.
    pub(super) async fn check_updates(&self, only: Option<&str>) -> Value {
        self.rescan().await;
        struct Candidate {
            id: String,
            name: String,
            dir: PathBuf,
            key: String,
            version: String,
            permissions: Vec<String>,
            hash: String,
            allowed: Value,
        }
        let decisions = lock(&self.inner.state).decisions.clone();
        let candidates: Vec<Candidate> = lock(&self.inner.entries)
            .iter()
            // (The store's are checked together, in one request, below.)
            .filter(|e| !e.item.builtin && !e.item.own && e.store.is_none() && only.is_none_or(|id| id == e.item.id))
            .filter_map(|e| {
                let key = e.key()?.to_owned();
                // Compared with what you allowed it, or what it is when
                // you haven't yet.
                let allowed = decisions
                    .get(&e.item.id)
                    .filter(|d| d.allowed)
                    .map_or_else(|| e.record.clone(), |d| d.allowed_record());
                Some(Candidate {
                    id: e.item.id.clone(),
                    name: e.item.name.clone(),
                    dir: e.inspected.dir.clone(),
                    key,
                    version: e.item.version.clone(),
                    permissions: e.item.permissions.clone(),
                    hash: e.item.hash.clone(),
                    allowed,
                })
            })
            .collect();
        let mut updates = Vec::new();
        let mut errors = Vec::new();
        let Ok(scratch) = self.scratch() else {
            return json!({ "updates": [], "errors": [{ "error": "Couldn't make room to check" }] });
        };
        for c in candidates {
            let (id, name) = (c.id.clone(), c.name.clone());
            let Some(source) = Source::load(&c.dir) else { continue };
            let plugins_dir = self.inner.plugins_dir.clone();
            let installed = install::Installed {
                id: &c.id,
                key: &c.key,
                version: &c.version,
                permissions: &c.permissions,
                hash: &c.hash,
                allowed: &c.allowed,
            };
            match install::fetch_again(&source, &scratch).await {
                Ok(Some((fetched, source))) => match install::compare(
                    &installed,
                    &fetched,
                    source,
                    fetched.key.as_deref() != Some(c.key.as_str()) && self.same_account(&c.key, fetched.key.as_deref().unwrap_or_default(), &c.id).await,
                ) {
                    Some(found) => {
                        let keep = found.refused.is_none().then_some(&fetched);
                        if let Err(error) = install::keep_found(&plugins_dir, &id, &found, keep) {
                            errors.push(json!({ "id": id, "name": name, "error": error }));
                            continue;
                        }
                        let mut shown = found.shown();
                        shown["id"] = json!(id);
                        shown["name"] = json!(name);
                        updates.push(shown);
                    }
                    None => install::forget_found(&plugins_dir, &id),
                },
                Ok(None) => install::forget_found(&plugins_dir, &id),
                // A tampered or unsigned new version is refused here: never kept.
                Err(error) => errors.push(json!({ "id": id, "name": name, "error": error })),
            }
        }
        let (store_updates, store_errors) = self.check_store_updates(only).await;
        updates.extend(store_updates);
        errors.extend(store_errors);
        self.rescan().await;
        json!({ "updates": updates, "errors": errors })
    }

    /// Check each template installed from a package where it came from (or
    /// `only`'s) for a new version, as plugins' are.
    pub(super) async fn check_template_updates(&self, only: Option<&str>) -> Value {
        let dir = self.inner.data_dir.join(super::TEMPLATES_DIR);
        let listed = {
            let dir = dir.clone();
            tokio::task::spawn_blocking(move || template_packages::checkable(&dir)).await.unwrap_or_default()
        };
        let mut updates = Vec::new();
        let mut errors = Vec::new();
        for (id, name, key, hash, source) in listed.into_iter().filter(|c| only.is_none_or(|o| o == c.0)) {
            match template_packages::fetch_again(&source).await {
                Ok(Some((fetched, source))) => match template_packages::compare(&dir, &id, &key, &hash, &fetched, source) {
                    Some(found) => {
                        let keep = found.refused.is_none().then_some(&fetched);
                        if let Err(error) = template_packages::keep_found(&dir, &id, &found, keep) {
                            errors.push(json!({ "id": id, "name": name, "error": error }));
                            continue;
                        }
                        let mut shown = found.shown();
                        shown["id"] = json!(id);
                        shown["name"] = json!(name);
                        updates.push(shown);
                    }
                    None => template_packages::forget_found(&dir, &id),
                },
                Ok(None) => template_packages::forget_found(&dir, &id),
                Err(error) => errors.push(json!({ "id": id, "name": name, "error": error })),
            }
        }
        json!({ "updates": updates, "errors": errors })
    }

    async fn update(&self, id: &str, verified_only: bool) -> Result<RpcReply, RpcError> {
        self.rescan().await;
        let entry = self.entry(id).ok_or_else(|| failed(format!("There's no plugin {id}")))?;
        if entry.item.builtin {
            return Err(failed("Built-in plugins update when Zeron does"));
        }
        // An update is the person's (or their other device's): never an
        // agent's, whatever let the call through.
        if !Self::by_person() {
            return Err(Self::needs_person(&entry.item.name));
        }
        // A plugin this device takes verified updates of takes nothing its
        // record doesn't say is verified, now: not when the store can't be
        // asked either.
        let verified_only = verified_only || (entry.store.is_some() && lock(&self.inner.state).updates_for(id) == super::status::UpdatePreference::Verified);
        let mut found = install::found(&self.inner.plugins_dir, id, &entry.item.hash);
        if found.is_none() {
            let checked = self.check_updates(Some(id)).await;
            if let Some(error) = checked["errors"].as_array().and_then(|e| e.first()) {
                return Err(failed(error["error"].as_str().unwrap_or("Couldn't check for an update").to_owned()));
            }
            found = install::found(&self.inner.plugins_dir, id, &entry.item.hash);
        }
        let Some(found) = found else {
            return Err(failed(install::failure(install::reason::UP_TO_DATE, format!("{} is up to date", entry.item.name))));
        };
        if let Some(refused) = found.refused {
            return Err(failed(refused));
        }
        let path = install::found_package_path(&self.inner.plugins_dir, id);
        // Unsigned only as the store's, when its record named no signer (a
        // deploy token's version); from anywhere else, signed.
        let signed = found.source.as_ref().and_then(|s| s.store()).is_none_or(|s| s.key.is_some());
        let package = blocking(move || package::read(&path, &install::WORDS).and_then(|raw| install::parse_from_store(raw, signed))).await?;
        if package.hash != found.hash {
            return Err(failed("The update changed since it was found; check again"));
        }
        let mut source = found
            .source
            .or_else(|| Source::load(Path::new(&entry.item.dir)))
            .ok_or_else(|| failed("Where it came from isn't known"))?;
        // From the store: whether it's verified is the store's record's,
        // asked now, never what the check remembered (unknown: not).
        if let Some(store) = source.store.as_mut().filter(|_| source.kind == install::STORE) {
            store.verified = self.verified_now(id, &found.version, &package.hash, package.key.as_deref()).await.map_err(failed)?;
            if verified_only && !store.verified {
                return Err(failed(install::failure(
                    install::reason::NOT_VERIFIED,
                    format!("{} {} isn't verified by Zeron: an update that isn't is taken on its own, from its sheet", entry.item.name, found.version),
                )));
            }
        }
        self.install_package(package, source).await
    }

    /// Plugin `id` installed again from where it came from: fetched and
    /// verified first, then put in place of the folder there (kept aside
    /// until the new one is in, and put back if it can't be). What it
    /// stored stays; your decision goes, so it asks again.
    async fn reinstall(&self, id: &str) -> Result<RpcReply, RpcError> {
        self.rescan().await;
        let entry = self.entry(id).ok_or_else(|| failed(format!("There's no plugin {id}")))?;
        // It starts over what the person allowed: theirs to do.
        if !Self::by_person() {
            return Err(failed(install::failure(
                install::reason::NEEDS_PERSON,
                format!("Installing {} again is the person's, in Zeron's window", entry.item.name),
            )));
        }
        if entry.item.builtin {
            return Err(failed("Built-in plugins update when Zeron does"));
        }
        if entry.item.own {
            return Err(failed("It's one you're making on this device: there's nowhere else to get it from"));
        }
        let dir = PathBuf::from(&entry.item.dir);
        let source = Source::load(&dir).ok_or_else(|| {
            failed(install::failure(
                install::reason::NO_SOURCE,
                "Where it came from isn't known, so it can't be installed again from there",
            ))
        })?;
        let (package, mut fresh) = if source.kind == install::STORE {
            // The version installed, again from the store (yanked or not:
            // it's yours already).
            self.fetch_from_store(id, Some(source.version.as_str()), true).await?
        } else {
            install::refetch(&source, &self.scratch()?).await.map_err(failed)?
        };
        if package.id() != id {
            return Err(failed(install::failure(
                install::reason::DIFFERENT_PLUGIN,
                format!("What's there now is a different plugin ({})", package.id()),
            )));
        }
        // (A git source keeps the folder in the repository it named.)
        if fresh.path.is_none() {
            fresh.path = source.path.clone();
        }
        let aside = self.inner.plugins_dir.join(install::INCOMING_DIR).join(format!("{id}-reinstall-{}", crate::new_id()));
        {
            let _scan = self.inner.scan_lock.lock().await;
            std::fs::rename(&dir, &aside).map_err(|_| failed("Couldn't replace the installed one"))?;
        }
        self.update_state(|state| {
            state.decisions.remove(id);
        })?;
        install::forget_found(&self.inner.plugins_dir, id);
        match self.install_package_keeping(package, fresh, true).await {
            Ok(reply) => {
                let _ = std::fs::remove_dir_all(&aside);
                Ok(reply)
            }
            Err(error) => {
                if std::fs::symlink_metadata(&dir).is_err() {
                    let _ = std::fs::rename(&aside, &dir);
                }
                self.rescan().await;
                Err(error)
            }
        }
    }

    /// Check for updates a while after starting, then every few hours.
    pub(super) fn spawn_update_checks(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let weak = std::sync::Arc::downgrade(&self.inner);
        let task = runtime.spawn(async move {
            tokio::time::sleep(FIRST_CHECK).await;
            loop {
                let Some(inner) = weak.upgrade() else { return };
                let plugins = Plugins { inner };
                let checked = plugins.check_updates(None).await;
                if let Some(n) = checked["updates"].as_array().map(Vec::len).filter(|n| *n > 0) {
                    tracing::info!(updates = n, "plugins: updates found");
                }
                let checked = plugins.check_template_updates(None).await;
                if let Some(n) = checked["updates"].as_array().map(Vec::len).filter(|n| *n > 0) {
                    tracing::info!(updates = n, "templates: updates found");
                }
                drop(plugins);
                tokio::time::sleep(CHECK_EVERY).await;
            }
        });
        lock(&self.inner.tasks).push(task);
        let weak = std::sync::Arc::downgrade(&self.inner);
        let (first, every) = takedown_every();
        let feed = runtime.spawn(async move {
            tokio::time::sleep(first).await;
            loop {
                let Some(inner) = weak.upgrade() else { return };
                let plugins = Plugins { inner };
                plugins.check_takedowns().await;
                plugins.check_publishes().await;
                drop(plugins);
                tokio::time::sleep(every).await;
            }
        });
        lock(&self.inner.tasks).push(feed);
    }
}

/// What a scan adds to an item from someone else: where it came from, and
/// the update found for it.
pub(super) fn annotate(plugins_dir: &Path, item: &mut super::scan::PluginItem) {
    if item.builtin {
        return;
    }
    let dir = Path::new(&item.dir);
    item.source = Source::load(dir).map(|s| s.shown());
    item.update = install::found(plugins_dir, &item.id, &item.hash).map(|f| f.shown());
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_take_down_interval_setting_only_ever_shortens() {
        let built_in = (FIRST_CHECK, TAKEDOWN_EVERY);
        assert_eq!(takedown_every_from(shorter(None)), built_in);
        // Zero, junk, or longer than built in: ignored.
        for ignored in ["0", "", "soon", "-5", "1.5", "901", "3600", "18446744073709551616"] {
            assert_eq!(shorter(Some(ignored)), None, "{ignored:?}");
        }
        // Shorter: taken, and the first read never waits longer than built in.
        assert_eq!(takedown_every_from(shorter(Some("1"))), (Duration::from_secs(1), Duration::from_secs(1)));
        assert_eq!(takedown_every_from(shorter(Some(" 600 "))), (FIRST_CHECK, Duration::from_secs(600)));
        assert_eq!(takedown_every_from(shorter(Some("900"))), (FIRST_CHECK, TAKEDOWN_EVERY));
        // An account with no plugins: looked at hourly unless a setting
        // shortens it (never by the feed's interval alone).
        assert_eq!(super::super::store::publishes_idle(None), Duration::from_secs(60 * 60));
        assert_eq!(super::super::store::publishes_idle(Some(Duration::from_secs(1))), Duration::from_secs(1));
    }
}
