//! `InstallPlugin`, `CheckPluginUpdates`, `UpdatePlugin`, `ExportPlugin`:
//! the IO around `install.rs`. Uninstalling is `RemovePlugin` (mod.rs): its
//! folder, its decision, what it stored, and the update kept for it.
//!
//! Consent stays the app's. Installing records a decision only for a plugin
//! signed by your own key (one of your devices'); anyone else's is listed
//! `pending`, and the app's consent window asks. An update signed by the
//! same key keeps what you allowed; one asking for more waits for you again
//! (`status::decide`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use zeron_rpc::{RpcError, RpcReply, methods, parse_params};

use super::install::{self, Package, Source};
use super::package;
use super::status::{Author, Decision, Status};
use super::{Plugins, failed, lock, now_secs};

/// The first check after the engine starts, then how often.
const FIRST_CHECK: Duration = Duration::from_secs(90);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

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
}

#[derive(Deserialize)]
struct CheckParams {
    #[serde(default)]
    id: Option<String>,
}

#[derive(Deserialize)]
struct UpdateParams {
    id: String,
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
        methods::INSTALL_PLUGIN | methods::CHECK_PLUGIN_UPDATES | methods::UPDATE_PLUGIN | methods::EXPORT_PLUGIN
    )
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
                let path = p.path.filter(|s| !s.trim().is_empty());
                let url = p.url.filter(|s| !s.trim().is_empty());
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
            methods::CHECK_PLUGIN_UPDATES => {
                let p: CheckParams = parse_params(params)?;
                RpcReply::value(&self.check_updates(p.id.as_deref()).await)
            }
            methods::UPDATE_PLUGIN => {
                let p: UpdateParams = parse_params(params)?;
                self.update(&p.id).await
            }
            methods::EXPORT_PLUGIN => {
                let p: ExportParams = parse_params(params)?;
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

    /// Install a verified package, keep the decisions right, and say what
    /// changed.
    async fn install_package(&self, package: Package, mut source: Source) -> Result<RpcReply, RpcError> {
        self.rescan().await;
        let id = package.id().to_owned();
        if let Some(entry) = self.entry(&id) {
            if entry.item.builtin {
                return Err(failed(super::manifest::RESERVED_ID_ERROR));
            }
            if entry.item.own {
                return Err(failed(format!(
                    "You're making a plugin called {id} on this device. Remove or rename it first to install this one."
                )));
            }
        }
        source.version = package.manifest.version.clone();
        source.installed_at = now_secs();
        let author = self.resolve_now(&package.key).await;
        let plugins_dir = self.inner.plugins_dir.clone();
        let staged = package.clone();
        let replaced = blocking(move || install::install(&plugins_dir, &staged, &source)).await?;
        let asked = package.manifest.permissions.clone();
        self.update_state(|state| match &replaced {
            None => {
                state.decisions.remove(&id);
                // Yours (signed by one of your devices): no asking.
                if author == Author::You {
                    let mut permissions = asked.clone();
                    permissions.sort();
                    permissions.dedup();
                    state.decisions.insert(
                        id.clone(),
                        Decision { allowed: true, author_key: Some(package.key.clone()), permissions, content_hash: None, off: false },
                    );
                }
            }
            Some(old) => {
                // Allowed by its content (an author not known): the update
                // is signed by the same key, so it's still the one allowed.
                if let Some(d) = state.decisions.get_mut(&id)
                    && d.allowed
                    && d.author_key.is_none()
                    && d.content_hash.as_deref() == Some(old.hash.as_str())
                {
                    d.content_hash = Some(package.hash.clone());
                }
            }
        })?;
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
        if let Some(old) = &replaced {
            reply["from"] = json!(old.version);
        }
        RpcReply::value(&reply)
    }

    /// Check every installed plugin's source (or `only`'s) for a new version.
    pub(super) async fn check_updates(&self, only: Option<&str>) -> Value {
        self.rescan().await;
        let candidates: Vec<(String, String, PathBuf, String, Vec<String>, String)> = lock(&self.inner.entries)
            .iter()
            .filter(|e| !e.item.builtin && !e.item.own && only.is_none_or(|id| id == e.item.id))
            .filter_map(|e| {
                let key = e.key()?.to_owned();
                Some((e.item.id.clone(), e.item.name.clone(), e.inspected.dir.clone(), key, e.item.permissions.clone(), e.item.hash.clone()))
            })
            .collect();
        let mut updates = Vec::new();
        let mut errors = Vec::new();
        let Ok(scratch) = self.scratch() else {
            return json!({ "updates": [], "errors": [{ "error": "Couldn't make room to check" }] });
        };
        for (id, name, dir, key, permissions, hash) in candidates {
            let Some(source) = Source::load(&dir) else { continue };
            let plugins_dir = self.inner.plugins_dir.clone();
            match install::fetch_again(&source, &scratch).await {
                Ok(Some((fetched, source))) => match install::compare(&id, &key, &permissions, &hash, &fetched, source) {
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
        self.rescan().await;
        json!({ "updates": updates, "errors": errors })
    }

    async fn update(&self, id: &str) -> Result<RpcReply, RpcError> {
        self.rescan().await;
        let entry = self.entry(id).ok_or_else(|| failed(format!("There's no plugin {id}")))?;
        if entry.item.builtin {
            return Err(failed("Zeron's own plugins update with Zeron"));
        }
        let mut found = install::found(&self.inner.plugins_dir, id, &entry.item.hash);
        if found.is_none() {
            let checked = self.check_updates(Some(id)).await;
            if let Some(error) = checked["errors"].as_array().and_then(|e| e.first()) {
                return Err(failed(error["error"].as_str().unwrap_or("Couldn't check for an update").to_owned()));
            }
            found = install::found(&self.inner.plugins_dir, id, &entry.item.hash);
        }
        let Some(found) = found else {
            return Err(failed(format!("{} is up to date", entry.item.name)));
        };
        if let Some(refused) = found.refused {
            return Err(failed(refused));
        }
        let path = install::found_package_path(&self.inner.plugins_dir, id);
        let package = blocking(move || package::read(&path, &install::WORDS).and_then(install::parse)).await?;
        if package.hash != found.hash {
            return Err(failed("The update changed since it was found; check again"));
        }
        let source = found
            .source
            .or_else(|| Source::load(Path::new(&entry.item.dir)))
            .ok_or_else(|| failed("Where it came from isn't known"))?;
        self.install_package(package, source).await
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
                drop(plugins);
                tokio::time::sleep(CHECK_EVERY).await;
            }
        });
        lock(&self.inner.tasks).push(task);
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

