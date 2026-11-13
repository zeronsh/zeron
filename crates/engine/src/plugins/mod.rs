//! Zeron plugins, engine side (the plugins contract, v1).
//!
//! Folders under `<data>/plugins/<id>/` are watched, hashed, checked against
//! their ed25519 signature and the user's decisions, and listed to the app
//! (`ListPlugins` / `WatchPlugins`). A plugin that is `on` gets an immutable
//! snapshot the app loads from. Own plugins are re-signed with this device's
//! key as they change. Tool calls from agents reach the app through a broker
//! (`CallPluginTool` → `ServePluginTools`), and plugins talk to the same
//! plugin on the account's other devices over the device links
//! (`PluginSend` → `PluginDeliver` → `WatchPluginChannel`).
//!
//! The pure parts live in submodules: [`manifest`], [`files`], [`signing`],
//! [`status`] (state file + decision table), [`scan`], [`keys`],
//! [`snapshot`], [`broker`], [`scaffold`]; [`templates`] stores whole-app
//! templates for agents. This file does the IO around them.

pub mod broker;
pub mod capability;
pub mod files;
pub mod guides;
pub mod install;
pub mod keys;
mod lifecycle;
pub mod manifest;
pub mod package;
pub mod reviews;
pub mod scaffold;
pub mod scan;
pub mod service;
pub mod signing;
pub mod snapshot;
pub mod status;
pub mod template_packages;
pub mod templates;
pub mod themes;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::stream::BoxStream;
use serde::Deserialize;
use tokio::sync::{mpsc, watch};
use zeron_rpc::{RpcError, RpcReply, methods, parse_params};

use broker::{CallError, Slot, ToolAnswer, ToolBroker};
use keys::{CachedKey, KeyCache, KeyDirectory};
use manifest::MANIFEST_FILE;
use scan::{Entry, Inspected};
use signing::{SigCheck, SigningKey};
use snapshot::Retention;
use service::{Plan, Refusal, Services, reason};
use status::{Author, State, Status};

pub const PLUGINS_DIR: &str = "plugins";
pub const TEMPLATES_DIR: &str = "templates";
pub const REMOVED_DIR: &str = ".removed";
/// `PluginSend`'s `data`, serialized (checked again where it lands).
pub const MAX_SEND_BYTES: usize = 64 * 1024;
/// A service call's `args` from another device, serialized.
pub const MAX_CALL_BYTES: usize = 1024 * 1024;
/// The author Zeron's own plugins say they are across devices: never a
/// key, so never the same as one.
pub const BUILTIN_AUTHOR: &str = "zeron";
/// How often this device looks for signing keys added on the account's
/// other devices, and rescans so authors looked up longer ago than
/// `keys::CACHE_TTL_SECS` are looked up again (a key revoked elsewhere
/// stops counting within the hour).
const NEW_KEYS_EVERY: Duration = Duration::from_secs(15 * 60);
/// A peer's word (`CheckPluginKey`) looks this device's key up at most this often.
const OWN_KEY_CHECK_APART: Duration = Duration::from_secs(5);
/// Until those checks can run (auth not ready, the edge not answering).
const KEYS_RETRY: Duration = Duration::from_secs(30);

const RESCAN_DEBOUNCE: Duration = Duration::from_millis(200);
const RESIGN_DEBOUNCE: Duration = Duration::from_millis(300);
/// A failed key lookup isn't retried sooner than this (sign-in resets it).
const LOOKUP_RETRY: Duration = Duration::from_secs(5 * 60);
const LOOKUP_NOW_TIMEOUT: Duration = Duration::from_secs(5);
const DELIVER_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a device's windows watch another's service events per `@watch`,
/// and how often this device renews its own while a window is open.
const WATCH_FOR: Duration = Duration::from_secs(10 * 60);
const WATCH_RENEW: Duration = Duration::from_secs(4 * 60);
/// The extra a call to another device waits for its link beyond the call's
/// own timeout (which the other device keeps).
const LINK_SLACK: Duration = Duration::from_secs(10);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn failed(message: impl Into<String>) -> RpcError {
    RpcError::Failed(message.into())
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The methods this module answers.
pub fn handles(method: &str) -> bool {
    matches!(
        method,
        methods::LIST_PLUGINS
            | methods::WATCH_PLUGINS
            | methods::DECIDE_PLUGIN
            | methods::REVIEW_PLUGIN
            | methods::REVIEW_TEMPLATE
            | methods::SET_PLUGIN_OFF
            | methods::SET_BUILTIN_PLUGINS
            | methods::REMOVE_PLUGIN
            | methods::NEW_PLUGIN
            | methods::SIGN_PLUGIN
            | methods::VERIFY_PLUGIN
            | methods::LIST_PLUGIN_TOOLS
            | methods::WATCH_PLUGIN_TOOLS
            | methods::CALL_PLUGIN_TOOL
            | methods::SERVE_PLUGIN_TOOLS
            | methods::PLUGIN_TOOL_RESULT
            | methods::PLUGIN_SEND
            | methods::PLUGIN_DELIVER
            | methods::CALL_PLUGIN_SERVICE
            | methods::SERVE_PLUGIN_CALL
            | methods::WATCH_PLUGIN_CHANNEL
            | methods::PLUGIN_SIGNING_KEY
            | methods::CHECK_PLUGIN_KEY
            | methods::LIST_TEMPLATES
            | methods::CREATE_TEMPLATE
            | methods::USE_TEMPLATE
            | methods::TEMPLATE_STATUS
            | methods::DELETE_TEMPLATE
            | methods::EXPORT_TEMPLATE
            | methods::IMPORT_TEMPLATE
            | methods::DECIDE_TEMPLATE
            | methods::CHECK_TEMPLATE_UPDATES
            | methods::UPDATE_TEMPLATE
            | methods::DESCRIBE_APP
            | methods::DESCRIBE_ELEMENTS
            | methods::PLUGIN_STATUS
            | methods::WATCH_THEMES
    ) || lifecycle::handles(method)
        || themes::handles(method)
}

/// How long `PluginStatus` waits for the window's account.
const STATUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// The map of where plugins can go, as the main window wrote it
/// (`DescribeElements`).
pub const ELEMENTS_FILE: &str = ".elements.json";

type AccountFn = Arc<dyn Fn() -> Option<String> + Send + Sync>;
/// Posts a plugin's notification: `(source, title, body, session)` → outcome.
type NotifyFn = Arc<dyn Fn(&str, &str, &str, Option<&str>) -> String + Send + Sync>;
/// This device's chats' and projects' folders (a service's `zeron.folders`).
type FoldersFn = Arc<dyn Fn() -> Vec<String> + Send + Sync>;

enum Kick {
    Rescan,
    Paths(Vec<PathBuf>),
}

/// Whether a watcher event may mean something changed. inotify reports
/// reads too (a file opened, or closed unwritten): counted, every read of a
/// plugin's files (a rescan's own, a check's) set off another rescan, and
/// kept the re-sign's debounce from ever going quiet.
fn is_change(event: &notify::Event) -> bool {
    use notify::event::{AccessKind, AccessMode, EventKind};
    match event.kind {
        EventKind::Access(kind) => kind == AccessKind::Close(AccessMode::Write),
        _ => true,
    }
}

/// The engine's plugin service. Cheap to clone.
#[derive(Clone)]
pub struct Plugins {
    inner: Arc<Inner>,
}

struct Inner {
    data_dir: PathBuf,
    plugins_dir: PathBuf,
    device_id: String,
    device_name: Mutex<String>,
    workspace: Mutex<Option<crate::WorkspaceHost>>,
    directory: Mutex<Option<Arc<dyn KeyDirectory>>>,
    account: Mutex<Option<AccountFn>>,
    links: Mutex<Option<Arc<zeron_rpc::LinkCache>>>,
    state: Mutex<State>,
    cache: Mutex<KeyCache>,
    lookup_failed: Mutex<HashMap<String, Instant>>,
    lookups: Mutex<HashSet<String>>,
    entries: Mutex<Vec<Entry>>,
    scanned: AtomicBool,
    list_tx: watch::Sender<serde_json::Value>,
    tools_tx: watch::Sender<serde_json::Value>,
    scan_lock: tokio::sync::Mutex<()>,
    retention: Mutex<Retention>,
    sweep_scheduled: AtomicBool,
    /// Manifest bytes the auto re-sign last wrote, per id.
    written: Mutex<HashMap<String, Vec<u8>>>,
    tools: Arc<ToolBroker>,
    channel: Arc<Slot>,
    /// The window, told when the theme methods change the theme files.
    themes: Arc<Slot>,
    kick: Mutex<Option<mpsc::UnboundedSender<Kick>>>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    watch: Mutex<Option<crate::fs_watch::FsWatch>>,
    /// The plugins dir as the watcher reports it (canonical).
    root: Mutex<Option<PathBuf>>,
    /// Zeron's own plugins, the app's folder of them (`SetBuiltinPlugins`).
    builtin_dir: Mutex<Option<PathBuf>>,
    /// Zeron's own plugins' folder, watched as yours are: an edit to one
    /// (developing Zeron) loads as it's saved, not at the next rescan.
    builtin_watch: Mutex<Option<crate::fs_watch::FsWatch>>,
    /// Plugins' services (plugins/service).
    services: Services,
    /// Devices whose windows watch this device's service events, by plugin:
    /// each until it renews.
    watchers: Mutex<HashMap<String, HashMap<String, Instant>>>,
    /// The (plugin, device) services this device's windows watch.
    watching: Mutex<HashSet<(String, String)>>,
    notifier: Mutex<Option<NotifyFn>>,
    /// Where this device's chats and projects are (`zeron.folders`).
    folders: Mutex<Option<FoldersFn>>,
    /// One-time capabilities for Zeron's consent window (`ReviewPlugin`,
    /// `ReviewTemplate`): what allowing and signing need.
    reviews: Mutex<reviews::Reviews>,
    /// The ticket of the sign-in just made (`Auth::step_up`), which
    /// registering a new key needs.
    step_up: Mutex<Option<StepUpFn>>,
    /// Signing keys added on the account's other devices that this device
    /// noticed (`PluginSigningKey`'s `new_keys`).
    new_keys: Mutex<Vec<keys::KeyRow>>,
    /// What the host says about the account's signing keys (a key added on
    /// another device, this device's revoked): `{id, kind, words}`, carried
    /// on the plugin list for the app's own strip, newest last.
    key_notices: Mutex<Vec<serde_json::Value>>,
    /// When this device's own key was last looked up on a peer's word
    /// (`CheckPluginKey`): at most every few seconds.
    own_key_checked: Mutex<Option<Instant>>,
}

type StepUpFn = Arc<dyn Fn() -> Option<String> + Send + Sync>;

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
struct DecideParams {
    id: String,
    allow: bool,
    hash: String,
    /// `ReviewPlugin`'s capability: needed to allow.
    #[serde(default)]
    review: Option<String>,
}

#[derive(Deserialize)]
struct ReviewParams {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    pending: Option<String>,
    hash: String,
}

#[derive(Deserialize)]
struct OffParams {
    id: String,
    off: bool,
}

#[derive(Deserialize)]
struct NewParams {
    id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    permissions: Vec<String>,
}

#[derive(Deserialize)]
struct SignParams {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    path: Option<String>,
    /// `ReviewPlugin`'s capability for the plugin signed.
    #[serde(default)]
    review: Option<String>,
}

#[derive(Deserialize)]
struct PathParams {
    path: String,
}

#[derive(Deserialize)]
struct CallParams {
    #[serde(default, alias = "mcpName")]
    mcp_name: Option<String>,
    #[serde(default)]
    plugin: Option<String>,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    args: serde_json::Value,
    #[serde(default, rename = "chatId", alias = "chat_id")]
    chat_id: Option<String>,
}

#[derive(Deserialize)]
struct ResultParams {
    #[serde(rename = "callId", alias = "call_id")]
    call_id: String,
    ok: bool,
    #[serde(default)]
    value: serde_json::Value,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct SendParams {
    plugin: String,
    to: String,
    #[serde(default)]
    data: serde_json::Value,
}

#[derive(Deserialize)]
struct DeliverParams {
    plugin: String,
    from: String,
    #[serde(default, alias = "authorKey")]
    author_key: Option<String>,
    #[serde(default)]
    data: serde_json::Value,
    /// A service's event (`zeron.emit`) for windows here, not a message.
    #[serde(default)]
    event: Option<String>,
    /// From another device whose id the link didn't say: `from` is only
    /// what it claimed.
    #[serde(skip)]
    unverified_from: bool,
}

#[derive(Deserialize)]
struct ServiceParams {
    plugin: String,
    #[serde(default)]
    method: String,
    #[serde(default)]
    args: serde_json::Value,
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    timeout: Option<f64>,
}

/// `ServePluginCall`'s params (and a call on this device, as one).
#[derive(Deserialize, serde::Serialize)]
struct ServeParams {
    plugin: String,
    #[serde(default)]
    version: String,
    #[serde(default, rename = "authorKey", alias = "author_key")]
    author_key: Option<String>,
    #[serde(default)]
    from: String,
    #[serde(default)]
    method: String,
    #[serde(default)]
    args: serde_json::Value,
    #[serde(default)]
    timeout: Option<f64>,
    /// From another device whose id the link didn't say.
    #[serde(skip)]
    unverified_from: bool,
}

/// `PluginDeliver` outcomes (and the sender's `offline`).
pub mod outcome {
    pub const DELIVERED: &str = "delivered";
    pub const NOT_INSTALLED: &str = "not_installed";
    pub const NOT_ALLOWED: &str = "not_allowed";
    pub const NOT_OPEN: &str = "not_open";
    pub const OFFLINE: &str = "offline";
    pub const TOO_BIG: &str = "too_big";
}

/// `<plugin>__<tool>`, with `-` → `_`.
pub fn mcp_name(plugin: &str, tool: &str) -> String {
    format!("{plugin}__{tool}").replace('-', "_")
}

/// Write `bytes` beside `path` and rename it over (readers never see half a
/// file). The temp name starts with a dot, so hashing and watching skip it.
fn write_replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let temp = dir.join(format!(".{name}.tmp-{}", crate::new_id()));
    let result = std::fs::write(&temp, bytes).and_then(|()| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Sign the plugin folder `dir` with `key` (its manifest rewritten in place).
/// Returns the bytes written, or `None` when it was signed by `key` already.
fn sign_folder(dir: &Path, key: &SigningKey) -> Result<Option<Vec<u8>>, String> {
    let inspected = scan::inspect(dir, false);
    let Some(manifest) = inspected.manifest.as_ref() else {
        return Err(inspected.error.unwrap_or_else(|| "It can't be read".into()));
    };
    if let Some(problem) = manifest.problem() {
        return Err(problem.to_owned());
    }
    if inspected.hash.is_empty() {
        return Err(inspected
            .error
            .unwrap_or_else(|| "Its files can't be read".into()));
    }
    if inspected.sig == SigCheck::Valid(key.public()) {
        return Ok(None);
    }
    let bytes = signing::pretty(&key.sign_manifest(&manifest.raw, &inspected.files));
    write_replace(&dir.join(MANIFEST_FILE), &bytes)
        .map_err(|_| "Couldn't write its zeron-plugin.json".to_string())?;
    Ok(Some(bytes))
}

impl Plugins {
    /// The service for the engine whose data dir is `data_dir`. Nothing runs
    /// until [`Self::start`].
    pub fn new(data_dir: &Path, device_id: &str) -> Self {
        Self::with_services(data_dir, device_id, service::Config::default())
    }

    /// [`Self::new`], its services run by `config` (tests: a short idle).
    pub fn with_services(data_dir: &Path, device_id: &str, config: service::Config) -> Self {
        let plugins_dir = data_dir.join(PLUGINS_DIR);
        let storage = plugins_dir.join(".storage");
        let state = State::load(&plugins_dir);
        let cache = KeyCache::load(data_dir);
        let empty = serde_json::json!({
            "dirs": {
                "plugins": plugins_dir.display().to_string(),
                "templates": data_dir.join(TEMPLATES_DIR).display().to_string(),
            },
            "plugins": [],
        });
        let (list_tx, _) = watch::channel(empty);
        let (tools_tx, _) = watch::channel(serde_json::Value::Null);
        let plugins = Self {
            inner: Arc::new(Inner {
                data_dir: data_dir.to_path_buf(),
                plugins_dir,
                device_id: device_id.to_owned(),
                device_name: Mutex::new(crate::local_device_name(device_id)),
                workspace: Mutex::new(None),
                directory: Mutex::new(None),
                account: Mutex::new(None),
                links: Mutex::new(None),
                state: Mutex::new(state),
                cache: Mutex::new(cache),
                lookup_failed: Mutex::new(HashMap::new()),
                lookups: Mutex::new(HashSet::new()),
                entries: Mutex::new(Vec::new()),
                scanned: AtomicBool::new(false),
                list_tx,
                tools_tx,
                scan_lock: tokio::sync::Mutex::new(()),
                retention: Mutex::new(Retention::default()),
                sweep_scheduled: AtomicBool::new(false),
                written: Mutex::new(HashMap::new()),
                reviews: Mutex::new(reviews::Reviews::default()),
                tools: Arc::new(ToolBroker::default()),
                channel: Arc::new(Slot::default()),
                themes: Arc::new(Slot::default()),
                kick: Mutex::new(None),
                tasks: Mutex::new(Vec::new()),
                watch: Mutex::new(None),
                root: Mutex::new(None),
                builtin_dir: Mutex::new(None),
                builtin_watch: Mutex::new(None),
                services: Services::new(&storage, config),
                watchers: Mutex::new(HashMap::new()),
                watching: Mutex::new(HashSet::new()),
                notifier: Mutex::new(None),
                folders: Mutex::new(None),
                step_up: Mutex::new(None),
                new_keys: Mutex::new(Vec::new()),
                key_notices: Mutex::new(Vec::new()),
                own_key_checked: Mutex::new(None),
            }),
        };
        plugins.inner.services.set_hooks(Arc::new(ServiceHooks {
            inner: Arc::downgrade(&plugins.inner),
        }));
        plugins.publish_tools();
        plugins
    }

    /// How a service's `zeron.notify` is posted (the engine's banners).
    pub fn set_notifier(
        &self,
        notify: impl Fn(&str, &str, &str, Option<&str>) -> String + Send + Sync + 'static,
    ) {
        *lock(&self.inner.notifier) = Some(Arc::new(notify));
    }

    /// Where this device's chats and projects are, for a service's
    /// `zeron.folders()` (the engine's workspace; a test's own).
    pub fn set_folders(&self, folders: impl Fn() -> Vec<String> + Send + Sync + 'static) {
        *lock(&self.inner.folders) = Some(Arc::new(folders));
    }

    pub fn plugins_dir(&self) -> &Path {
        &self.inner.plugins_dir
    }

    /// Where key lookups and registration go (the edge, or a stand-in).
    pub fn set_directory(&self, directory: Arc<dyn KeyDirectory>) {
        *lock(&self.inner.directory) = Some(directory);
    }

    /// Who is signed in (`None` = signed out). Key lookups are cached per account.
    pub fn set_account(&self, account: impl Fn() -> Option<String> + Send + Sync + 'static) {
        *lock(&self.inner.account) = Some(Arc::new(account));
    }

    pub fn set_workspace(&self, workspace: crate::WorkspaceHost) {
        *lock(&self.inner.workspace) = Some(workspace);
    }

    pub fn set_links(&self, links: Arc<zeron_rpc::LinkCache>) {
        *lock(&self.inner.links) = Some(links);
    }

    /// Follow `auth`: the account for lookups, and on every sign-in change a
    /// key registration and a rescan.
    pub fn attach_auth(&self, auth: crate::Auth) {
        let for_account = auth.clone();
        self.set_account(move || for_account.user_id());
        let for_step_up = auth.clone();
        *lock(&self.inner.step_up) = Some(Arc::new(move || for_step_up.step_up()));
        // Signed in on an engine with no edge of its own (local mode, where
        // the first sign-in happens): its keys are looked up and registered
        // with the edge it signs in against.
        if self.directory().is_none() && auth.workos_enabled() {
            self.set_directory(Arc::new(keys::EdgeDirectory::new(auth.edge_url(), Arc::new(auth.clone()))));
        }
        // A fresh ticket (a sign-in, before it's announced and the app
        // restarts the engine; a step-up): this device's key registered with
        // it while it's here.
        // Through the edge it signs in against, with the session's own
        // bearer: the engine may have no edge of its own (local mode, where
        // the first sign-in happens), and the sign-in isn't announced yet.
        let weak = Arc::downgrade(&self.inner);
        let fresh: Arc<dyn KeyDirectory> = Arc::new(keys::EdgeDirectory::new(auth.edge_url(), auth.session_bearer()));
        auth.on_fresh_ticket(Arc::new(move |account: String| {
            let (weak, fresh) = (weak.clone(), fresh.clone());
            Box::pin(async move {
                if let Some(inner) = weak.upgrade() {
                    Plugins { inner }.register_key_through(fresh.as_ref(), &account).await;
                }
            })
        }));
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let weak = Arc::downgrade(&self.inner);
        let mut states = auth.watch_state();
        let task = runtime.spawn(async move {
            // Once for the account as it is now, then on every change.
            let mut first = true;
            loop {
                if !std::mem::take(&mut first) && states.changed().await.is_err() {
                    return;
                }
                let Some(inner) = weak.upgrade() else { return };
                let plugins = Plugins { inner };
                lock(&plugins.inner.lookup_failed).clear();
                plugins.settle_revokes().await;
                plugins.register_key().await;
                plugins.check_keys().await;
                plugins.kick(Kick::Rescan);
            }
        });
        lock(&self.inner.tasks).push(task);
    }

    fn account(&self) -> Option<String> {
        let account = lock(&self.inner.account).clone();
        account.and_then(|f| f())
    }

    fn directory(&self) -> Option<Arc<dyn KeyDirectory>> {
        lock(&self.inner.directory).clone()
    }

    fn device_name(&self, device_id: &str) -> String {
        let workspace = lock(&self.inner.workspace).clone();
        let known = workspace
            .and_then(|ws| ws.read_devices().ok())
            .and_then(|devices| devices.into_iter().find(|d| d.id == device_id))
            .map(|d| d.name)
            .filter(|name| !name.trim().is_empty());
        match known {
            Some(name) => name,
            None if device_id == self.inner.device_id => lock(&self.inner.device_name).clone(),
            None => device_id.to_owned(),
        }
    }

    /// Watch the folder, run the debounce worker, scan once, register the key.
    pub fn start(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if let Err(err) = std::fs::create_dir_all(&self.inner.plugins_dir) {
            tracing::warn!(error = %err, "plugins: couldn't make the plugins folder");
        }
        let (tx, rx) = mpsc::unbounded_channel();
        *lock(&self.inner.kick) = Some(tx.clone());
        let worker = runtime.spawn(Self::worker(Arc::downgrade(&self.inner), rx));
        let this = self.clone();
        let setup = runtime.spawn(async move {
            let watcher = this.clone();
            let _ = tokio::task::spawn_blocking(move || watcher.install_watch(tx)).await;
            this.register_key().await;
            this.rescan().await;
        });
        // This device's windows' watches on other devices' services, renewed
        // while a window is open.
        let weak = Arc::downgrade(&self.inner);
        let renew = runtime.spawn(async move {
            loop {
                tokio::time::sleep(WATCH_RENEW).await;
                let Some(inner) = weak.upgrade() else { return };
                let plugins = Plugins { inner };
                if !plugins.inner.channel.is_open() {
                    continue;
                }
                let watching: Vec<(String, String)> = lock(&plugins.inner.watching).iter().cloned().collect();
                for (plugin, device) in watching {
                    let this = plugins.clone();
                    tokio::spawn(async move {
                        this.call_service(ServiceParams {
                            plugin,
                            method: "@watch".into(),
                            args: serde_json::Value::Null,
                            device: Some(device),
                            timeout: None,
                        })
                        .await;
                    });
                }
            }
        });
        let weak = Arc::downgrade(&self.inner);
        // At start (as soon as auth and the edge answer: retried every
        // half minute until they do), then every 15 minutes.
        let new_keys = runtime.spawn(async move {
            loop {
                let Some(inner) = weak.upgrade() else { return };
                let plugins = Plugins { inner };
                let checked = plugins.check_keys().await;
                plugins.kick(Kick::Rescan);
                drop(plugins);
                tokio::time::sleep(if checked { NEW_KEYS_EVERY } else { KEYS_RETRY }).await;
            }
        });
        lock(&self.inner.tasks).extend([worker, setup, renew, new_keys]);
        self.spawn_update_checks();
    }

    fn install_watch(&self, tx: mpsc::UnboundedSender<Kick>) {
        let root = std::fs::canonicalize(&self.inner.plugins_dir)
            .unwrap_or_else(|_| self.inner.plugins_dir.clone());
        *lock(&self.inner.root) = Some(root);
        let watch = crate::fs_watch::FsWatchHub::global().watch(
            &self.inner.plugins_dir,
            notify::RecursiveMode::Recursive,
            move |event: &notify::Event| {
                if !is_change(event) {
                    return;
                }
                let kick = if event.need_rescan() || event.paths.is_empty() {
                    Kick::Rescan
                } else {
                    Kick::Paths(event.paths.clone())
                };
                let _ = tx.send(kick);
            },
        );
        *lock(&self.inner.watch) = Some(watch);
    }

    /// Watches Zeron's own plugins' folder: any change there rescans (the
    /// worker's debounce), so a built-in plugin edited in a checkout
    /// reloads as it's saved. In an installed app the folder never changes.
    fn watch_builtin(&self, dir: &Path) {
        let Some(tx) = lock(&self.inner.kick).clone() else { return };
        let watch = crate::fs_watch::FsWatchHub::global().watch(dir, notify::RecursiveMode::Recursive, move |event: &notify::Event| {
            if is_change(event) {
                let _ = tx.send(Kick::Rescan);
            }
        });
        *lock(&self.inner.builtin_watch) = Some(watch);
    }

    pub fn shutdown(&self) {
        lock(&self.inner.watch).take();
        lock(&self.inner.builtin_watch).take();
        lock(&self.inner.kick).take();
        for task in lock(&self.inner.tasks).drain(..) {
            task.abort();
        }
        self.inner.tools.drop_pending();
        self.inner.services.shutdown();
    }

    fn kick(&self, kick: Kick) {
        if let Some(tx) = lock(&self.inner.kick).as_ref() {
            let _ = tx.send(kick);
        }
    }

    /// Debounce: rescan 200 ms after the last change (300 ms when an own
    /// plugin is re-signed first).
    async fn worker(weak: Weak<Inner>, mut rx: mpsc::UnboundedReceiver<Kick>) {
        while let Some(first) = rx.recv().await {
            let Some(inner) = weak.upgrade() else { return };
            let plugins = Plugins { inner };
            let mut scan = false;
            let mut resign: HashSet<String> = HashSet::new();
            plugins.note(first, &mut scan, &mut resign);
            loop {
                let wait = if resign.is_empty() {
                    RESCAN_DEBOUNCE
                } else {
                    RESIGN_DEBOUNCE
                };
                match tokio::time::timeout(wait, rx.recv()).await {
                    Ok(Some(kick)) => plugins.note(kick, &mut scan, &mut resign),
                    Ok(None) => return,
                    Err(_) => break,
                }
            }
            if !resign.is_empty() {
                let this = plugins.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    for id in resign {
                        this.auto_resign(&id);
                    }
                })
                .await;
                scan = true;
            }
            if scan {
                plugins.rescan().await;
            }
        }
    }

    /// Fold one watcher event into the pending work.
    fn note(&self, kick: Kick, scan: &mut bool, resign: &mut HashSet<String>) {
        let paths = match kick {
            Kick::Rescan => {
                *scan = true;
                return;
            }
            Kick::Paths(paths) => paths,
        };
        let root = lock(&self.inner.root).clone();
        for path in paths {
            let rel = root
                .as_deref()
                .and_then(|root| path.strip_prefix(root).ok())
                .or_else(|| path.strip_prefix(&self.inner.plugins_dir).ok());
            let Some(rel) = rel else { continue };
            let parts: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            let Some(id) = parts.first() else {
                *scan = true;
                continue;
            };
            // `.loaded`, `.removed`, `.state.json`, dotfiles inside a plugin.
            if parts.iter().any(|part| part.starts_with('.')) {
                continue;
            }
            *scan = true;
            if !lock(&self.inner.state).own.contains(id) {
                continue;
            }
            if parts.len() == 2 && parts[1] == MANIFEST_FILE {
                let written = lock(&self.inner.written).get(id).cloned();
                if written.is_some_and(|bytes| std::fs::read(&path).is_ok_and(|now| now == bytes)) {
                    // Our own write.
                    continue;
                }
            }
            resign.insert(id.clone());
        }
    }

    /// Re-sign an own plugin after it changed (never one that isn't own).
    fn auto_resign(&self, id: &str) {
        if !lock(&self.inner.state).own.contains(id) {
            return;
        }
        let Some(key) = signing::load_key(&self.inner.data_dir) else {
            return;
        };
        match sign_folder(&self.inner.plugins_dir.join(id), &key) {
            Ok(Some(bytes)) => {
                tracing::debug!(plugin = %id, "plugins: re-signed");
                lock(&self.inner.written).insert(id.to_owned(), bytes);
            }
            Ok(None) => {}
            // Broken right now (a half-saved manifest): it shows as broken.
            Err(error) => tracing::debug!(plugin = %id, %error, "plugins: not re-signed"),
        }
    }

    fn resign_all_own(&self) {
        let own: Vec<String> = lock(&self.inner.state).own.iter().cloned().collect();
        for id in own {
            self.auto_resign(&id);
        }
    }

    /// Register this device's key for the signed-in account (retried next
    /// start, or next sign-in, when it fails).
    async fn register_key(&self) {
        if let Some(account) = self.account() {
            self.register_key_as(&account).await;
        }
    }

    /// [`Self::register_key`] in the background, when this device's key
    /// isn't registered for the account signed in.
    fn spawn_register(&self) {
        let (Some(account), Ok(runtime)) = (self.account(), tokio::runtime::Handle::try_current()) else {
            return;
        };
        let registered = signing::load_key(&self.inner.data_dir)
            .is_some_and(|key| keys::Registration::load(&self.inner.data_dir).covers(&key.public(), &account));
        if !registered {
            let this = self.clone();
            runtime.spawn(async move { this.register_key().await });
        }
    }

    /// [`Self::register_key`] for `account` (a sign-in not yet announced).
    async fn register_key_as(&self, account: &str) {
        if let Some(directory) = self.directory() {
            self.register_key_through(directory.as_ref(), account).await;
        }
    }

    /// Register this device's key for `account` through `directory`.
    async fn register_key_through(&self, directory: &dyn KeyDirectory, account: &str) {
        let account = account.to_owned();
        let step_up = lock(&self.inner.step_up).clone().and_then(|f| f());
        match keys::ensure_registered(
            &self.inner.data_dir,
            directory,
            &self.inner.device_id,
            &account,
            step_up.as_deref(),
        )
        .await
        {
            Ok(true) => {
                let this = self.clone();
                let _ = tokio::task::spawn_blocking(move || this.resign_all_own()).await;
                self.kick(Kick::Rescan);
            }
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "plugins: signing key not registered yet"),
        }
    }

    /// Revoke the keys owed a revoke on the account signed in now.
    async fn settle_revokes(&self) {
        if let (Some(directory), Some(account)) = (self.directory(), self.account()) {
            keys::settle_revokes(&self.inner.data_dir, directory.as_ref(), &account).await;
        }
    }

    /// Before signing out: this device's key revoked on the edge (or owed
    /// a revoke when it can't be reached) and its private half deleted, so
    /// nobody signs as this account with it again. (`SignOut`, with the
    /// session still there to revoke with.)
    pub async fn retire_signing_key(&self) {
        let directory = self.directory();
        keys::retire_key(&self.inner.data_dir, directory.as_deref(), self.account().as_deref()).await;
        lock(&self.inner.new_keys).clear();
    }

    /// Look for signing keys added on the account's other devices since
    /// last time; each is posted as a notice and kept for `PluginSigningKey`.
    async fn look_for_new_keys(&self) -> bool {
        let (Some(directory), Some(account)) = (self.directory(), self.account()) else {
            return false;
        };
        match keys::new_keys_elsewhere(&self.inner.data_dir, directory.as_ref(), &self.inner.device_id, &account).await {
            Ok(rows) => {
                for row in &rows {
                    let named = self.device_name(&row.device_id);
                    let device = if named == row.device_id || named.trim().is_empty() { "another device".to_owned() } else { named };
                    tracing::info!(device = %row.device_id, fingerprint = %row.fingerprint, "plugins: a new signing key on the account");
                    self.key_notice(
                        &format!("key-added:{}", row.key),
                        "key_added",
                        Some(&device),
                        "A new signing key",
                        &format!(
                            "A new signing key was added to your account on {device}. If that wasn't you, revoke it in Settings → Account. (Fingerprint {}.)",
                            row.fingerprint
                        ),
                    );
                }
                lock(&self.inner.new_keys).extend(rows);
                true
            }
            Err(error) => {
                tracing::info!(%error, "plugins: couldn't look for new signing keys yet");
                false
            }
        }
    }

    /// Whether this device's key is still registered: one revoked from
    /// another device (or gone from the edge) stops being yours at once. The
    /// key is deleted, a fresh one made (registered with the next sign-in's
    /// ticket), plugins it signed checked again like anyone's, your own
    /// plugins re-signed and asked about again, and you're told.
    async fn check_own_key(&self) -> bool {
        let (Some(directory), Some(account)) = (self.directory(), self.account()) else {
            return false;
        };
        let Some(key) = signing::load_key(&self.inner.data_dir).map(|k| k.public()) else {
            return true;
        };
        let registered = keys::Registration::load(&self.inner.data_dir).covers(&key, &account);
        *lock(&self.inner.own_key_checked) = Some(Instant::now());
        let revoked = match directory.lookup(&key).await {
            // Revoked on this account (whatever this device's file says).
            Ok(keys::Lookup::Found { revoked: true, mine: true, .. }) => true,
            // Registered here, and now gone or someone else's.
            Ok(keys::Lookup::Found { mine: false, .. }) | Ok(keys::Lookup::Unknown) => registered,
            Ok(keys::Lookup::Found { .. }) => false,
            Err(error) => {
                tracing::info!(%error, "plugins: couldn't check this device's signing key yet");
                return false;
            }
        };
        if revoked {
            self.own_key_revoked(&key).await;
        }
        true
    }

    async fn own_key_revoked(&self, key: &str) {
        tracing::warn!("plugins: this device's signing key was revoked elsewhere; making a new one");
        // Revoked already: nothing to revoke, just never sign with it again.
        signing::delete_key(&self.inner.data_dir);
        let _ = std::fs::remove_file(self.inner.data_dir.join(keys::REGISTRATION_FILE));
        let cached = {
            let mut cache = lock(&self.inner.cache);
            if let Some(account) = self.account() {
                cache.0.insert(key.to_owned(), keys::CachedKey::from_lookup(&keys::Lookup::Unknown, &account, now_secs()));
            }
            cache.clone()
        };
        cached.save(&self.inner.data_dir);
        // What runs outside their sandbox was allowed your own plugins as
        // you: asked again (what stays inside it is kept).
        let _ = self.update_state(|state| {
            let own: Vec<String> = state.own.iter().cloned().collect();
            for id in own {
                if let Some(d) = state.decisions.get_mut(&id) {
                    let inside = capability::inside_sandbox(&d.allowed_record());
                    d.permissions.retain(|p| !capability::OUTSIDE_SANDBOX.contains(&p.as_str()));
                    d.grants = Some(inside);
                }
            }
        });
        let this = self.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if signing::load_or_create_key(&this.inner.data_dir).is_ok() {
                this.resign_all_own();
            }
        })
        .await;
        lock(&self.inner.new_keys).retain(|row| row.key != key);
        let here = if cfg!(target_os = "macos") { "This Mac's" } else { "This device's" };
        self.key_notice(
            &format!("key-revoked:{key}"),
            "own_key_revoked",
            None,
            "Signing key revoked",
            &format!(
                "{here} signing key was revoked from another device. Plugins it signed no longer count as yours; your own plugins' programs, services and other code-running powers wait for you to allow them again. Sign in again to register a new key."
            ),
        );
        self.register_key().await;
        self.kick(Kick::Rescan);
    }

    /// Tell `device` a key was revoked, so it checks its own now rather
    /// than at its next regular check (best effort: offline, it checks then).
    fn tell_key_revoked(&self, device: String) {
        let (Some(links), Ok(runtime)) = (lock(&self.inner.links).clone(), tokio::runtime::Handle::try_current()) else {
            return;
        };
        runtime.spawn(async move {
            let told = tokio::time::timeout(DELIVER_TIMEOUT, async {
                links.client(&device).await?.call(methods::CHECK_PLUGIN_KEY, serde_json::json!({})).await
            })
            .await;
            if !matches!(told, Ok(Ok(_))) {
                tracing::info!(%device, "plugins: couldn't tell the device its key was revoked; it checks on its own");
            }
        });
    }

    /// Said by the host (the app's toasts, from the plugin list, in its
    /// own words from `kind` and `device`, the other device's name) and as
    /// a desktop banner: once per `id`.
    fn key_notice(&self, id: &str, kind: &str, device: Option<&str>, title: &str, words: &str) {
        {
            let mut notices = lock(&self.inner.key_notices);
            if notices.iter().any(|n| n["id"] == id) {
                return;
            }
            notices.push(serde_json::json!({ "id": id, "kind": kind, "device": device, "words": words }));
            let excess = notices.len().saturating_sub(20);
            notices.drain(..excess);
        }
        self.publish_list();
        if let Some(notify) = lock(&self.inner.notifier).clone() {
            notify("zeron", title, words, None);
        }
    }

    /// A peer said it revoked a key (`CheckPluginKey`): this device's own
    /// looked up now, unless it was a moment ago.
    async fn check_own_key_soon(&self) {
        let recent = lock(&self.inner.own_key_checked).is_some_and(|at| at.elapsed() < OWN_KEY_CHECK_APART);
        if !recent {
            self.check_own_key().await;
        }
    }

    /// The account's key checks: this device's own key's standing, and keys
    /// added on other devices. Whether they could run (signed in, edge up).
    async fn check_keys(&self) -> bool {
        let own = self.check_own_key().await;
        let others = self.look_for_new_keys().await;
        own && others
    }

    // ── Scanning ────────────────────────────────────────────────────────────

    fn scan_blocking(&self) -> (Vec<Entry>, Vec<String>, bool) {
        self.prune_state();
        let state = lock(&self.inner.state).clone();
        let cache = lock(&self.inner.cache).clone();
        let failed = lock(&self.inner.lookup_failed).clone();
        let own_key = signing::load_key(&self.inner.data_dir).map(|k| k.public());
        let account = self.account();
        let can_look_up = self.directory().is_some() && account.is_some();
        let now = now_secs();
        let mut owed = Vec::new();
        let mut entries: Vec<Entry> = scan::plugin_dirs(&self.inner.plugins_dir)
            .into_iter()
            .map(|dir| {
                let inspected = scan::inspect(&dir, true);
                let own = state.own.contains(&inspected.folder);
                let author = match inspected.signature_author() {
                    _ if own => Author::You,
                    Ok(author) => author,
                    Err(key) => {
                        let gave_up = !can_look_up || failed.contains_key(&key);
                        match keys::resolve_cached(
                            &cache,
                            &key,
                            own_key.as_deref(),
                            account.as_deref(),
                            now,
                            gave_up,
                        ) {
                            Ok(author) => author,
                            Err(author) => {
                                owed.push(key);
                                author
                            }
                        }
                    }
                };
                scan::entry(inspected, author, &state)
            })
            .collect();
        // Zeron's own, from the app's folder of them. A folder of yours by
        // the same name gives way, said so.
        if let Some(dir) = lock(&self.inner.builtin_dir).clone() {
            for dir in scan::plugin_dirs(&dir) {
                let inspected = scan::inspect(&dir, true);
                let id = inspected.folder.clone();
                let off = state.decisions.get(&id).is_some_and(|d| d.off);
                for theirs in entries.iter_mut().filter(|e| e.item.id == id) {
                    theirs.item.status = Status::Broken;
                    theirs.item.error = Some(manifest::RESERVED_ID_ERROR.to_string());
                }
                entries.push(scan::builtin_entry(inspected, off));
            }
        }
        for entry in entries.iter_mut() {
            lifecycle::annotate(&self.inner.plugins_dir, &mut entry.item);
        }
        entries.sort_by(|a, b| {
            (a.item.name.to_lowercase(), &a.item.id).cmp(&(b.item.name.to_lowercase(), &b.item.id))
        });
        let mut current = HashMap::new();
        for entry in entries.iter_mut().filter(|e| e.item.status == Status::On) {
            let inspected = &entry.inspected;
            match snapshot::ensure(
                &self.inner.plugins_dir,
                &entry.item.id,
                &inspected.dir,
                &inspected.hash,
                &inspected.manifest_bytes,
                &inspected.files,
            ) {
                Ok(path) => {
                    current.insert(entry.item.id.clone(), inspected.hash[..16].to_owned());
                    entry.item.snapshot = Some(path.display().to_string());
                }
                // It changed while it was copied: the change's own rescan follows.
                Err(error) => {
                    tracing::debug!(plugin = %entry.item.id, %error, "plugins: no snapshot yet")
                }
            }
        }
        let keeps =
            lock(&self.inner.retention).sweep(&self.inner.plugins_dir, current, Instant::now());
        (entries, owed, keeps)
    }

    /// Forget own ids and decisions whose folder is gone (yours, or Zeron's
    /// own for its off switch). Under the scan lock, which installs hold
    /// while they swap a folder.
    fn prune_state(&self) {
        let names = |dir: &Path| -> Vec<String> {
            scan::plugin_dirs(dir)
                .iter()
                .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect()
        };
        let mut present: std::collections::BTreeSet<String> = names(&self.inner.plugins_dir).into_iter().collect();
        if let Some(dir) = lock(&self.inner.builtin_dir).clone() {
            present.extend(names(&dir));
        }
        let pruned = {
            let mut state = lock(&self.inner.state);
            state.prune(&present).then(|| state.clone())
        };
        if let Some(state) = pruned {
            tracing::info!("plugins: forgot decisions for plugins that are gone");
            if let Err(error) = state.save(&self.inner.plugins_dir) {
                tracing::warn!(%error, "plugins: couldn't save pruned choices");
            }
        }
    }

    /// Scan every plugin folder and push the list (and tools) if it changed.
    pub async fn rescan(&self) {
        let _guard = self.inner.scan_lock.lock().await;
        let this = self.clone();
        let Ok((entries, owed, keeps)) =
            tokio::task::spawn_blocking(move || this.scan_blocking()).await
        else {
            return;
        };
        *lock(&self.inner.entries) = entries;
        self.inner.scanned.store(true, Ordering::SeqCst);
        self.publish_list();
        self.publish_tools();
        self.sync_services();
        self.spawn_lookups(owed);
        if keeps && !self.inner.sweep_scheduled.swap(true, Ordering::SeqCst) {
            let weak = Arc::downgrade(&self.inner);
            tokio::spawn(async move {
                tokio::time::sleep(snapshot::KEEP_PREVIOUS + Duration::from_secs(1)).await;
                if let Some(inner) = weak.upgrade() {
                    inner.sweep_scheduled.store(false, Ordering::SeqCst);
                    Plugins { inner }.kick(Kick::Rescan);
                }
            });
        }
    }

    async fn ensure_scanned(&self) {
        if !self.inner.scanned.load(Ordering::SeqCst) {
            self.rescan().await;
        }
    }

    fn list_value(&self) -> serde_json::Value {
        let entries = lock(&self.inner.entries);
        let items: Vec<&scan::PluginItem> = entries.iter().map(|e| &e.item).collect();
        let key_notices = lock(&self.inner.key_notices).clone();
        serde_json::json!({
            "dirs": {
                "plugins": self.inner.plugins_dir.display().to_string(),
                "templates": self.inner.data_dir.join(TEMPLATES_DIR).display().to_string(),
            },
            "plugins": items,
            "key_notices": key_notices,
        })
    }

    fn publish_list(&self) {
        let value = self.list_value();
        self.inner.list_tx.send_if_modified(|current| {
            if *current == value {
                return false;
            }
            *current = value;
            true
        });
    }

    fn tools_value(&self) -> serde_json::Value {
        let window = self.inner.tools.serve.is_open();
        let entries = lock(&self.inner.entries);
        let tools: Vec<serde_json::Value> = entries
            .iter()
            .filter(|e| e.allows("tools"))
            .flat_map(|e| {
                // A plugin with a service answers with no window open.
                let open = window || service_plan(e).is_some();
                e.item.tools.iter().map(move |tool| {
                    serde_json::json!({
                        "plugin": e.item.id,
                        "plugin_name": e.item.name,
                        "tool": tool.name,
                        "mcp_name": mcp_name(&e.item.id, &tool.name),
                        "description": tool.description,
                        "input": tool.input,
                        "open": open,
                    })
                })
            })
            .collect();
        drop(entries);
        serde_json::json!({
            "device": { "id": self.inner.device_id, "name": self.device_name(&self.inner.device_id) },
            "tools": tools,
        })
    }

    fn publish_tools(&self) {
        let value = self.tools_value();
        self.inner.tools_tx.send_if_modified(|current| {
            if *current == value {
                return false;
            }
            *current = value;
            true
        });
    }

    fn spawn_lookups(&self, keys_owed: Vec<String>) {
        let (Some(directory), Some(account)) = (self.directory(), self.account()) else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        for key in keys_owed {
            let recently_failed = lock(&self.inner.lookup_failed)
                .get(&key)
                .is_some_and(|at| at.elapsed() < LOOKUP_RETRY);
            if recently_failed || !lock(&self.inner.lookups).insert(key.clone()) {
                continue;
            }
            let this = self.clone();
            let directory = directory.clone();
            let account = account.clone();
            runtime.spawn(async move {
                this.look_up(directory.as_ref(), &key, &account).await;
                lock(&this.inner.lookups).remove(&key);
                this.kick(Kick::Rescan);
            });
        }
    }

    /// One lookup; the result lands in the cache (or the failure list).
    async fn look_up(
        &self,
        directory: &dyn KeyDirectory,
        key: &str,
        account: &str,
    ) -> Option<CachedKey> {
        match directory.lookup(key).await {
            Ok(lookup) => {
                let entry = CachedKey::from_lookup(&lookup, account, now_secs());
                let cache = {
                    let mut cache = lock(&self.inner.cache);
                    cache.0.insert(key.to_owned(), entry.clone());
                    cache.clone()
                };
                cache.save(&self.inner.data_dir);
                lock(&self.inner.lookup_failed).remove(key);
                Some(entry)
            }
            Err(error) => {
                tracing::debug!(%error, "plugins: key lookup failed");
                lock(&self.inner.lookup_failed).insert(key.to_owned(), Instant::now());
                None
            }
        }
    }

    /// Who signed with `key`, as the scan says it (own key, account, the
    /// `plugin-keys.json` cache, the lookups given up on), without waiting:
    /// a key still owed is looked up, so the next ask answers better. For a
    /// template package's signer (plugins/template_packages.rs).
    pub fn author_of(&self, key: &str) -> Author {
        let cache = lock(&self.inner.cache).clone();
        let own_key = signing::load_key(&self.inner.data_dir).map(|k| k.public());
        let account = self.account();
        let gave_up = self.directory().is_none()
            || account.is_none()
            || lock(&self.inner.lookup_failed).contains_key(key);
        let resolved = keys::resolve_cached(&cache, key, own_key.as_deref(), account.as_deref(), now_secs(), gave_up);
        if resolved.is_err() {
            self.spawn_lookups(vec![key.to_owned()]);
        }
        // What a template's install is decided by: stale or unchecked is
        // Unknown, never the last author known.
        keys::for_granting(resolved, gave_up)
    }

    /// Who signed with `key`, waiting (briefly) on the edge if need be.
    /// `ListTemplates`' reply with who signed each template from someone
    /// else (as known now), and the packages waiting for you, each with its
    /// signer.
    fn with_template_authors(&self, mut reply: serde_json::Value) -> serde_json::Value {
        let Some(list) = reply.get_mut("templates").and_then(|t| t.as_array_mut()) else {
            return reply;
        };
        let dir = self.inner.data_dir.join(TEMPLATES_DIR);
        for entry in list.iter_mut() {
            if let Some(key) = entry["imported"]["key"].as_str().map(str::to_owned) {
                entry["imported"]["author"] = serde_json::json!(self.author_of(&key));
            }
            // Where it came from (never a local path), and a newer version
            // found there.
            if let Some(source) = entry["imported"]
                .as_object_mut()
                .and_then(|i| i.remove("source"))
                .and_then(|s| serde_json::from_value::<install::Source>(s).ok())
            {
                entry["imported"]["source"] = source.shown();
            }
            if let (Some(id), Some(hash)) = (entry["id"].as_str(), entry["imported"]["hash"].as_str())
                && let Some(found) = template_packages::found(&dir, id, hash)
            {
                entry["update"] = found.shown();
            }
        }
        let pending: Vec<serde_json::Value> = template_packages::pending(&dir)
            .into_iter()
            .map(|mut p| {
                if let Some(key) = p["key"].as_str().map(str::to_owned) {
                    p["author"] = serde_json::json!(self.author_of(&key));
                }
                p
            })
            .collect();
        reply["pending"] = serde_json::json!(pending);
        reply
    }

    async fn resolve_now(&self, key: &str) -> Author {
        let own_key = signing::load_key(&self.inner.data_dir).map(|k| k.public());
        let account = self.account();
        let cached = {
            let cache = lock(&self.inner.cache);
            keys::resolve_cached(
                &cache,
                key,
                own_key.as_deref(),
                account.as_deref(),
                now_secs(),
                false,
            )
        };
        if let Ok(author) = cached {
            return author;
        }
        let (Some(directory), Some(account)) = (self.directory(), account) else {
            return Author::unknown(keys::COULD_NOT_CHECK);
        };
        match tokio::time::timeout(
            LOOKUP_NOW_TIMEOUT,
            self.look_up(directory.as_ref(), key, &account),
        )
        .await
        {
            Ok(Some(entry)) => entry.author(),
            // Stale or offline: Unknown, never the last author known (a
            // revoked key would keep passing for its owner).
            _ => Author::unknown(keys::COULD_NOT_CHECK),
        }
    }

    fn entry(&self, id: &str) -> Option<Entry> {
        lock(&self.inner.entries)
            .iter()
            .find(|e| e.item.id == id)
            .cloned()
    }

    fn item(&self, id: &str) -> Result<RpcReply, RpcError> {
        let entry = self
            .entry(id)
            .ok_or_else(|| failed(format!("There's no plugin {id}")))?;
        RpcReply::value(&entry.item)
    }

    fn update_state(&self, change: impl FnOnce(&mut State)) -> Result<(), RpcError> {
        let state = {
            let mut state = lock(&self.inner.state);
            change(&mut state);
            state.clone()
        };
        std::fs::create_dir_all(&self.inner.plugins_dir)
            .map_err(|_| failed("Couldn't save your plugin choices"))?;
        state.save(&self.inner.plugins_dir).map_err(failed)
    }

    /// Native MCP servers for a local run: plugins `on`, with `native`
    /// allowed and an `mcp` entry, each started from its snapshot.
    pub fn native_mcp_servers(&self) -> Vec<zeron_proto::McpServer> {
        lock(&self.inner.entries)
            .iter()
            .filter(|e| e.allows("native"))
            .filter_map(|e| {
                let mcp = e.mcp()?;
                let snapshot = e.item.snapshot.as_ref()?;
                let command = match mcp.command.strip_prefix("./") {
                    // (The manifest's path is `/`-separated: joined part by
                    // part, it's this platform's own, `\` on Windows.)
                    Some(rest) => rest
                        .split('/')
                        .filter(|part| !part.is_empty())
                        .fold(PathBuf::from(snapshot), |path, part| path.join(part))
                        .display()
                        .to_string(),
                    None => mcp.command.clone(),
                };
                Some(zeron_proto::McpServer {
                    name: format!("plugin-{}", e.item.id),
                    command,
                    args: mcp.args.clone(),
                    env: mcp.env.clone(),
                })
            })
            .collect()
    }

    /// Use up a consent-window review for `subject` as shown (`hash`):
    /// refused to anything but this device's own app.
    fn redeem_review(
        &self,
        kind: reviews::Kind,
        subject: &str,
        hash: &str,
        review: Option<&str>,
    ) -> Result<(), RpcError> {
        if !zeron_rpc::caller().is_local() {
            return Err(reviews::needed(match kind {
                reviews::Kind::Plugin => "Allowing a plugin",
                reviews::Kind::Template => "Allowing a template",
            }));
        }
        lock(&self.inner.reviews).redeem(kind, subject, hash, review)
    }

    /// `SignPlugin`'s review: for the plugin `id` names, or `path` when it's
    /// a folder in the plugins folder, as it is now. Anything else (a folder
    /// elsewhere, no review) is refused.
    async fn redeem_sign_review(
        &self,
        id: Option<&str>,
        path: Option<&str>,
        review: Option<&str>,
    ) -> Result<(), RpcError> {
        let id = match (id, path) {
            (Some(id), _) => Some(id.to_owned()),
            (None, Some(path)) => {
                let canonical = std::fs::canonicalize(path).ok();
                let root = std::fs::canonicalize(&self.inner.plugins_dir).ok();
                canonical
                    .filter(|c| root.is_some() && c.parent() == root.as_deref())
                    .and_then(|c| c.file_name().map(|n| n.to_string_lossy().into_owned()))
            }
            (None, None) => None,
        };
        let Some(id) = id else {
            return Err(failed("Only a plugin in your plugins folder can be signed, by its id"));
        };
        self.rescan().await;
        let hash = self.entry(&id).map(|e| e.item.hash).unwrap_or_default();
        self.redeem_review(reviews::Kind::Plugin, &id, &hash, review)
    }

    // ── Methods ─────────────────────────────────────────────────────────────

    pub async fn dispatch(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<RpcReply, RpcError> {
        let params = if params.is_null() {
            serde_json::json!({})
        } else {
            params
        };
        match method {
            methods::LIST_PLUGINS => {
                self.rescan().await;
                RpcReply::value(&self.list_value())
            }
            methods::REVIEW_PLUGIN | methods::REVIEW_TEMPLATE => {
                let p: ReviewParams = parse_params(params)?;
                if !zeron_rpc::caller().is_local() {
                    return Err(reviews::needed("A review"));
                }
                let review = if method == methods::REVIEW_PLUGIN {
                    let id = p.id.unwrap_or_default();
                    self.rescan().await;
                    let entry = self
                        .entry(&id)
                        .ok_or_else(|| failed(format!("There's no plugin {id}")))?;
                    if entry.item.hash != p.hash {
                        return Err(failed("It changed while you were looking; review it again"));
                    }
                    lock(&self.inner.reviews).mint(reviews::Kind::Plugin, &id, &p.hash)?
                } else {
                    let pending = p.pending.unwrap_or_default();
                    lock(&self.inner.reviews).mint(reviews::Kind::Template, pending.trim(), &p.hash)?
                };
                RpcReply::value(&serde_json::json!({ "review": review }))
            }
            methods::WATCH_PLUGINS => {
                self.ensure_scanned().await;
                Ok(RpcReply::Stream(watch_stream(
                    self.inner.list_tx.subscribe(),
                )))
            }
            methods::DECIDE_PLUGIN => {
                let p: DecideParams = parse_params(params)?;
                // Allowing is Zeron's consent window's alone: this device's
                // app, with a review minted for this plugin as shown.
                if p.allow {
                    // (Changed since it was shown: say so, keeping the review.)
                    self.rescan().await;
                    if self.entry(&p.id).is_some_and(|e| e.item.hash != p.hash) {
                        return Err(failed("It changed while you were looking; review it again"));
                    }
                    self.redeem_review(reviews::Kind::Plugin, &p.id, &p.hash, p.review.as_deref())?;
                }
                self.rescan().await;
                let entry = self
                    .entry(&p.id)
                    .ok_or_else(|| failed(format!("There's no plugin {}", p.id)))?;
                if entry.item.hash != p.hash {
                    return Err(failed("It changed while you were looking; review it again"));
                }
                if entry.item.status == Status::Broken {
                    return Err(failed(
                        entry
                            .item
                            .error
                            .unwrap_or_else(|| "It can't be turned on".into()),
                    ));
                }
                // Allowed for who made it as known now: a stale or offline
                // lookup is Unknown, and what was shown must still hold.
                if p.allow
                    && !entry.item.own
                    && !entry.item.builtin
                    && let Some(key) = entry.key()
                    && self.resolve_now(key).await != entry.item.author
                {
                    self.rescan().await;
                    return Err(failed("Who made it couldn't be confirmed just now; review it again"));
                }
                let decision = entry.facts().record(p.allow);
                self.update_state(|state| {
                    state.decisions.insert(p.id.clone(), decision);
                })?;
                self.rescan().await;
                self.item(&p.id)
            }
            methods::SET_BUILTIN_PLUGINS => {
                #[derive(serde::Deserialize)]
                struct BuiltinParams {
                    dir: String,
                }
                let p: BuiltinParams = parse_params(params)?;
                let dir = PathBuf::from(p.dir.trim());
                if !dir.is_absolute() || !dir.is_dir() {
                    return Err(failed("Zeron's own plugins: that isn't a folder"));
                }
                *lock(&self.inner.builtin_dir) = Some(dir.clone());
                self.watch_builtin(&dir);
                self.rescan().await;
                RpcReply::value(&self.list_value())
            }
            methods::SET_PLUGIN_OFF => {
                let p: OffParams = parse_params(params)?;
                self.rescan().await;
                let entry = self
                    .entry(&p.id)
                    .ok_or_else(|| failed(format!("There's no plugin {}", p.id)))?;
                // Zeron's own: on or off is all there is to decide.
                if entry.item.builtin {
                    self.update_state(|state| {
                        let mut decision = state.decisions.get(&p.id).cloned().unwrap_or_else(|| entry.facts().record(true));
                        decision.off = p.off;
                        state.decisions.insert(p.id.clone(), decision);
                    })?;
                    self.rescan().await;
                    return self.item(&p.id);
                }
                if !matches!(entry.item.status, Status::On | Status::Off) {
                    return Err(failed(format!("Review {} first", entry.item.name)));
                }
                let existing = lock(&self.inner.state).decisions.get(&p.id).cloned();
                let mut decision = match existing {
                    Some(decision) if decision.allowed => decision,
                    Some(_) => return Err(failed(format!("Allow {} first", entry.item.name))),
                    // Own and asking for nothing: on without a decision.
                    None => entry.facts().record(true),
                };
                decision.off = p.off;
                self.update_state(|state| {
                    state.decisions.insert(p.id.clone(), decision);
                })?;
                self.rescan().await;
                self.item(&p.id)
            }
            methods::REMOVE_PLUGIN => {
                let p: IdParams = parse_params(params)?;
                if self.entry(&p.id).is_some_and(|e| e.item.builtin) {
                    return Err(failed("Built-in plugins can be turned off, but not removed"));
                }
                // (One plain name, joined so it stays in the plugins folder on
                // every system: never `C:`, a device name or a trailing dot.)
                let dir = match zeron_plugin_guard::path::join(&self.inner.plugins_dir, &p.id) {
                    Ok(dir) if !p.id.starts_with('.') && !p.id.contains('/') => dir,
                    _ => return Err(failed(format!("There's no plugin {}", p.id))),
                };
                if std::fs::symlink_metadata(&dir).is_err() {
                    return Err(failed(format!("There's no plugin {}", p.id)));
                }
                let removed = self.inner.plugins_dir.join(REMOVED_DIR);
                std::fs::create_dir_all(&removed).map_err(|_| failed("Couldn't remove it"))?;
                std::fs::rename(&dir, removed.join(format!("{}-{}", p.id, now_secs())))
                    .map_err(|_| failed("Couldn't remove it"))?;
                self.update_state(|state| {
                    state.decisions.remove(&p.id);
                    state.own.remove(&p.id);
                })?;
                // What it kept (`zeron.store`, the app's) goes with it.
                let _ = std::fs::remove_file(self.inner.plugins_dir.join(".storage").join(format!("{}.json", p.id)));
                // And its settings (the app's: what you set on its page).
                let _ = std::fs::remove_file(self.inner.plugins_dir.join(".storage").join(format!("{}.settings.json", p.id)));
                // Its service's (`zeron.store` in service.luau).
                let _ = std::fs::remove_file(Services::store_path(&self.inner.plugins_dir.join(".storage"), &p.id));
                // Its database (`zeron.db`), and SQLite's files beside it.
                for end in ["sqlite3", "sqlite3-wal", "sqlite3-shm"] {
                    let _ = std::fs::remove_file(self.inner.plugins_dir.join(".storage").join(format!("{}.{end}", p.id)));
                }
                install::forget_found(&self.inner.plugins_dir, &p.id);
                self.rescan().await;
                RpcReply::value(&serde_json::json!({}))
            }
            methods::NEW_PLUGIN => {
                let p: NewParams = parse_params(params)?;
                manifest::check_id(&p.id).map_err(failed)?;
                if p.name.trim().is_empty() {
                    return Err(failed("It needs a name"));
                }
                if let Some(unknown) = p
                    .permissions
                    .iter()
                    .find(|x| !manifest::KNOWN_PERMISSIONS.contains(&x.as_str()))
                {
                    return Err(failed(format!(
                        "\"{unknown}\" isn't a permission Zeron knows"
                    )));
                }
                let dir = self.inner.plugins_dir.join(&p.id);
                let this = self.clone();
                let id = p.id.clone();
                tokio::task::spawn_blocking(move || -> Result<(), String> {
                    std::fs::create_dir_all(&this.inner.plugins_dir)
                        .map_err(|_| "Couldn't make the plugins folder".to_string())?;
                    scaffold::write(
                        &dir,
                        &id,
                        p.name.trim(),
                        p.description.as_deref().unwrap_or(""),
                        &p.permissions,
                    )?;
                    // Its README, linking the guide (signed with the rest).
                    let (guide, _) = guides::ensure(&this.inner.data_dir);
                    let _ = std::fs::write(
                        dir.join(guides::README),
                        guides::plugin_readme(p.name.trim(), &guide),
                    );
                    // Own before it's signed: the watcher's re-sign sees it as own.
                    let key = signing::load_or_create_key(&this.inner.data_dir)?;
                    this.update_state(|state| {
                        state.own.insert(id.clone());
                    })
                    .map_err(|e| e.to_string())?;
                    if let Some(bytes) = sign_folder(&dir, &key)? {
                        lock(&this.inner.written).insert(id, bytes);
                    }
                    Ok(())
                })
                .await
                .map_err(|_| failed("Couldn't make it"))?
                .map_err(failed)?;
                self.rescan().await;
                let entry = self
                    .entry(&p.id)
                    .ok_or_else(|| failed(format!("There's no plugin {}", p.id)))?;
                let mut item = serde_json::to_value(&entry.item).unwrap_or_default();
                let (guide, _) = guides::ensure(&self.inner.data_dir);
                item["guide"] = serde_json::json!(guide.display().to_string());
                item["guide_summary"] = serde_json::json!(guides::PLUGINS_SUMMARY);
                RpcReply::value(&item)
            }
            methods::SIGN_PLUGIN => {
                let p: SignParams = parse_params(params)?;
                // Signing makes a plugin yours: the consent window's alone,
                // with a review minted for this plugin as shown. Only a
                // plugin in the plugins folder can be signed.
                self.redeem_sign_review(p.id.as_deref(), p.path.as_deref(), p.review.as_deref())
                    .await?;
                // Only a folder in the plugins folder: this device's key
                // signs nothing anywhere else (it would vouch for it as you).
                let not_here = || failed("Only a plugin in your plugins folder can be signed");
                let id = match (p.id, p.path) {
                    (Some(id), _) => id,
                    (None, Some(path)) => {
                        let canonical = std::fs::canonicalize(PathBuf::from(path))
                            .map_err(|_| failed("That folder can't be found"))?;
                        let root = std::fs::canonicalize(&self.inner.plugins_dir).ok();
                        if root.is_none() || canonical.parent() != root.as_deref() {
                            return Err(not_here());
                        }
                        canonical
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .ok_or_else(not_here)?
                    }
                    (None, None) => return Err(failed("Say which plugin to sign")),
                };
                manifest::check_id(&id).map_err(failed)?;
                let dir = self.inner.plugins_dir.join(&id);
                if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
                    return Err(failed("That folder can't be found"));
                }
                if self.entry(&id).is_some_and(|e| e.item.builtin) {
                    return Err(failed(manifest::RESERVED_ID_ERROR));
                }
                let id = Some(id);
                let this = self.clone();
                let id_for_sign = id.clone();
                let dir_for_sign = dir.clone();
                let files = tokio::task::spawn_blocking(move || -> Result<usize, String> {
                    let key = signing::load_or_create_key(&this.inner.data_dir)?;
                    if let Some(id) = &id_for_sign {
                        let own = lock(&this.inner.state).own.contains(id);
                        if !own {
                            // Someone else's, signed and intact: never taken
                            // over (it would keep what they were allowed, as
                            // yours, with whatever was changed in it).
                            if let SigCheck::Valid(theirs) = scan::inspect(&dir_for_sign, true).sig
                                && theirs != key.public()
                            {
                                return Err("It's signed by someone else, so it can't be made yours. Copy it under a new id to make your own version.".into());
                            }
                        }
                        this.update_state(|state| {
                            // Taken as yours: what was allowed for whoever it
                            // was before is asked again.
                            if !own {
                                state.decisions.remove(id);
                            }
                            state.own.insert(id.clone());
                        })
                        .map_err(|e| e.to_string())?;
                    }
                    let written = sign_folder(&dir_for_sign, &key)?;
                    if let (Some(id), Some(bytes)) = (&id_for_sign, written) {
                        lock(&this.inner.written).insert(id.clone(), bytes);
                    }
                    Ok(files::hash_files(&dir_for_sign)
                        .map(|f| f.len())
                        .unwrap_or(0))
                })
                .await
                .map_err(|_| failed("Couldn't sign it"))?
                .map_err(failed)?;
                // Signed with a key the edge doesn't know yet: registered
                // now if a fresh sign-in's ticket is here (else at the next).
                self.spawn_register();
                match id {
                    Some(id) => {
                        self.rescan().await;
                        let mut reply = serde_json::to_value(
                            self.entry(&id)
                                .ok_or_else(|| failed(format!("There's no plugin {id}")))?
                                .item,
                        )
                        .unwrap_or_default();
                        reply["files"] = serde_json::json!(files);
                        Ok(RpcReply::Value(reply))
                    }
                    None => {
                        let mut reply = self.verify(&dir).await;
                        reply["files"] = serde_json::json!(files);
                        Ok(RpcReply::Value(reply))
                    }
                }
            }
            methods::VERIFY_PLUGIN => {
                let p: PathParams = parse_params(params)?;
                let dir = PathBuf::from(&p.path);
                if !dir.is_dir() {
                    return Err(failed("That folder can't be found"));
                }
                Ok(RpcReply::Value(self.verify(&dir).await))
            }
            methods::LIST_PLUGIN_TOOLS => {
                self.ensure_scanned().await;
                self.publish_tools();
                RpcReply::value(&self.tools_value())
            }
            methods::WATCH_PLUGIN_TOOLS => {
                self.ensure_scanned().await;
                self.publish_tools();
                Ok(RpcReply::Stream(watch_stream(
                    self.inner.tools_tx.subscribe(),
                )))
            }
            methods::CALL_PLUGIN_TOOL => {
                let p: CallParams = parse_params(params)?;
                self.call_tool(p).await
            }
            methods::SERVE_PLUGIN_TOOLS => {
                let weak = Arc::downgrade(&self.inner);
                let stream = self.inner.tools.serve.subscribe(move || {
                    if let Some(inner) = weak.upgrade() {
                        inner.tools.drop_pending();
                        Plugins { inner }.publish_tools();
                    }
                });
                self.publish_tools();
                Ok(RpcReply::Stream(stream))
            }
            methods::PLUGIN_TOOL_RESULT => {
                let p: ResultParams = parse_params(params)?;
                let answer = if p.ok {
                    ToolAnswer::Ok(p.value)
                } else {
                    ToolAnswer::Err(p.error.unwrap_or_else(|| "The tool failed".into()))
                };
                self.inner.tools.answer(&p.call_id, answer);
                RpcReply::value(&serde_json::json!({}))
            }
            methods::PLUGIN_SEND => {
                let p: SendParams = parse_params(params)?;
                self.send(p).await
            }
            methods::PLUGIN_DELIVER => {
                let mut p: DeliverParams = parse_params(params)?;
                p.from = self.trusted_from(p.from, &mut p.unverified_from);
                let outcome = self.deliver(p).await;
                RpcReply::value(&serde_json::json!({ "outcome": outcome }))
            }
            methods::CALL_PLUGIN_SERVICE => {
                let p: ServiceParams = parse_params(params)?;
                RpcReply::value(&self.call_service(p).await)
            }
            methods::SERVE_PLUGIN_CALL => {
                let mut p: ServeParams = parse_params(params)?;
                p.from = self.trusted_from(p.from, &mut p.unverified_from);
                RpcReply::value(&self.serve_call(p).await)
            }
            methods::WATCH_PLUGIN_CHANNEL => {
                Ok(RpcReply::Stream(self.inner.channel.subscribe(|| {})))
            }
            methods::WATCH_THEMES => Ok(RpcReply::Stream(self.inner.themes.subscribe(|| {}))),
            method if themes::handles(method) => {
                if let Some(by) = zeron_rpc::caller().record() {
                    tracing::info!(method, %by, "themes: asked by an agent");
                }
                let (data_dir, asked) = (self.inner.data_dir.clone(), method.to_owned());
                let reply = tokio::task::spawn_blocking(move || themes::answer(&data_dir, &asked, &params))
                    .await
                    .map_err(|_| failed("Couldn't reach the themes"))?
                    .map_err(failed)?;
                if themes::changes(method) {
                    self.inner.themes.push(serde_json::json!({ "changed": true }));
                }
                RpcReply::value(&reply)
            }
            methods::LIST_TEMPLATES
            | methods::CREATE_TEMPLATE
            | methods::USE_TEMPLATE
            | methods::TEMPLATE_STATUS
            | methods::DELETE_TEMPLATE
            | methods::DESCRIBE_APP => {
                #[derive(serde::Deserialize, Default)]
                struct TemplateParams {
                    #[serde(default)]
                    id: Option<String>,
                    #[serde(default)]
                    name: Option<String>,
                    #[serde(default)]
                    from: Option<String>,
                }
                let p: TemplateParams = parse_params(params)?;
                let dir = self.inner.data_dir.join(TEMPLATES_DIR);
                let data_dir = self.inner.data_dir.clone();
                // Who asked, when it wasn't this device's app (for the record).
                let by = zeron_rpc::caller().record();
                if let Some(by) = &by {
                    tracing::info!(method, id = ?p.id, %by, "templates: asked by an agent or another device");
                }
                let method = method.to_owned();
                let reply = tokio::task::spawn_blocking(move || {
                    let id = p.id.as_deref().map(str::trim).filter(|id| !id.is_empty());
                    match (method.as_str(), id) {
                        (methods::LIST_TEMPLATES, _) => templates::list(&dir),
                        (methods::DESCRIBE_APP, _) => templates::describe(&dir).map(|mut v| {
                            let (_, guide) = guides::ensure(&data_dir);
                            v["guide"] = serde_json::json!(guide.display().to_string());
                            v["guide_summary"] = serde_json::json!(guides::TEMPLATES_SUMMARY);
                            v
                        }),
                        (_, None) => {
                            Err("Say which template: its id, from list_templates.".to_string())
                        }
                        (methods::CREATE_TEMPLATE, Some(id)) => {
                            let name = p.name.as_deref().map(str::trim).filter(|n| !n.is_empty());
                            let (_, guide) = guides::ensure(&data_dir);
                            templates::create(&dir, id, name.unwrap_or(id), p.from.as_deref(), &guide).map(|mut v| {
                                v["guide"] = serde_json::json!(guide.display().to_string());
                                v["guide_summary"] = serde_json::json!(guides::TEMPLATES_SUMMARY);
                                v
                            })
                        }
                        (methods::USE_TEMPLATE, Some(id)) => templates::use_template(&dir, id, by.as_ref()),
                        (methods::TEMPLATE_STATUS, Some(id)) => templates::status(&dir, id),
                        (_, Some(id)) => templates::delete(&dir, id),
                    }
                })
                .await
                .map_err(|_| failed("Couldn't reach the templates folder"))?
                .map_err(failed)?;
                RpcReply::value(&self.with_template_authors(reply))
            }
            // From a link: fetched and verified, then imported as a file is.
            methods::IMPORT_TEMPLATE if params.get("url").and_then(|u| u.as_str()).is_some_and(|u| !u.trim().is_empty()) => {
                if params.get("path").and_then(|p| p.as_str()).is_some_and(|p| !p.trim().is_empty()) {
                    return Err(failed("Give a template file (`path`) or a link (`url`), not both"));
                }
                let url = params["url"].as_str().unwrap_or("").to_owned();
                let origin = template_packages::origin(&url).map_err(failed)?;
                let (package, mut source) = template_packages::fetch(&origin).await.map_err(failed)?;
                source.version = package.version();
                source.installed_at = now_secs();
                let from = source.url.clone().unwrap_or_default();
                let dir = self.inner.data_dir.join(TEMPLATES_DIR);
                let this = self.clone();
                let reply = tokio::task::spawn_blocking(move || {
                    template_packages::import_package(&dir, package, &from, source, |key: &str| this.author_of(key))
                })
                .await
                .map_err(|_| failed("Couldn't reach the templates folder"))?
                .map_err(failed)?;
                RpcReply::value(&reply)
            }
            methods::CHECK_TEMPLATE_UPDATES => {
                #[derive(serde::Deserialize, Default)]
                struct CheckParams {
                    #[serde(default)]
                    id: Option<String>,
                }
                let p: CheckParams = parse_params(params)?;
                RpcReply::value(&self.check_template_updates(p.id.as_deref()).await)
            }
            methods::UPDATE_TEMPLATE => {
                #[derive(serde::Deserialize)]
                struct UpdateParams {
                    id: String,
                }
                let p: UpdateParams = parse_params(params)?;
                let dir = self.inner.data_dir.join(TEMPLATES_DIR);
                let this = self.clone();
                let reply = tokio::task::spawn_blocking(move || {
                    template_packages::update(&dir, p.id.trim(), |key: &str| this.author_of(key))
                })
                .await
                .map_err(|_| failed("Couldn't reach the templates folder"))?
                .map_err(failed)?;
                RpcReply::value(&reply)
            }
            methods::EXPORT_TEMPLATE | methods::IMPORT_TEMPLATE | methods::DECIDE_TEMPLATE => {
                #[derive(serde::Deserialize, Default)]
                struct PackageParams {
                    #[serde(default)]
                    id: Option<String>,
                    #[serde(default)]
                    path: Option<String>,
                    #[serde(default)]
                    pending: Option<String>,
                    #[serde(default)]
                    allow: bool,
                    #[serde(default)]
                    hash: Option<String>,
                    #[serde(default)]
                    review: Option<String>,
                }
                let p: PackageParams = parse_params(params)?;
                // Allowing a template is Zeron's template consent window's
                // alone, with a review minted for this package as shown.
                if method == methods::DECIDE_TEMPLATE && p.allow {
                    let pending = p.pending.as_deref().unwrap_or_default().trim();
                    self.redeem_review(
                        reviews::Kind::Template,
                        pending,
                        p.hash.as_deref().unwrap_or_default(),
                        p.review.as_deref(),
                    )?;
                }
                // R29: an agent exports to Zeron's exports folder only; a
                // path of its choosing is the person's (their save dialog).
                if method == methods::EXPORT_TEMPLATE
                    && !zeron_rpc::caller().is_local()
                    && p.path.as_deref().is_some_and(|path| !path.trim().is_empty())
                {
                    return Err(failed(
                        "Agents export to Zeron's exports folder: leave out `path` (the reply says where it went)",
                    ));
                }
                let dir = self.inner.data_dir.join(TEMPLATES_DIR);
                let this = self.clone();
                let method = method.to_owned();
                let reply = tokio::task::spawn_blocking(move || {
                    let path = p.path.as_deref().map(str::trim).filter(|p| !p.is_empty()).map(PathBuf::from);
                    let author_of = |key: &str| this.author_of(key);
                    match method.as_str() {
                        methods::EXPORT_TEMPLATE => {
                            let id = p.id.as_deref().map(str::trim).filter(|id| !id.is_empty());
                            let Some(id) = id else {
                                return Err("Say which template (`id`).".to_string());
                            };
                            // Where it's asked to go, if that's a folder of
                            // yours; else Zeron's exports folder.
                            let path = match path {
                                Some(path) => {
                                    template_packages::check_export_path(&path, &this.inner.data_dir, Some(&crate::repos::home_dir()))?;
                                    path
                                }
                                None => {
                                    let path = template_packages::default_export_path(&this.inner.data_dir, id);
                                    if let Some(parent) = path.parent() {
                                        std::fs::create_dir_all(parent).map_err(|e| format!("Couldn't make its folder: {e}"))?;
                                    }
                                    path
                                }
                            };
                            let key = signing::load_or_create_key(&this.inner.data_dir)?;
                            template_packages::export(&dir, id, &path, &key)
                        }
                        methods::IMPORT_TEMPLATE => {
                            let path = path.ok_or("Say which file to import (`path`, a .zeron-template file).")?;
                            template_packages::import(&dir, &path, author_of)
                        }
                        _ => {
                            let pending = p.pending.unwrap_or_default();
                            template_packages::decide(&dir, pending.trim(), p.allow, p.hash.as_deref().unwrap_or(""), author_of)
                        }
                    }
                })
                .await
                .map_err(|_| failed("Couldn't reach the templates folder"))?
                .map_err(failed)?;
                RpcReply::value(&reply)
            }
            methods::PLUGIN_STATUS => {
                #[derive(serde::Deserialize)]
                struct StatusParams {
                    id: String,
                    #[serde(default)]
                    snapshot: bool,
                }
                let p: StatusParams = parse_params(params)?;
                self.rescan().await;
                let entry = self
                    .entry(&p.id)
                    .ok_or_else(|| failed(format!("There's no plugin {}", p.id)))?;
                let item = serde_json::to_value(&entry.item).unwrap_or_default();
                let mut out = serde_json::json!({ "id": entry.item.id, "name": entry.item.name });
                for key in ["status", "error", "permissions", "allowed", "dir", "version"] {
                    if let Some(v) = item.get(key).filter(|v| !v.is_null()) {
                        out[key] = v.clone();
                    }
                }
                // Its service here: running, what it serves, its last log lines.
                if entry.item.service.is_some() {
                    let status = self.inner.services.status(&entry.item.id);
                    out["service"] = if status.is_null() { serde_json::json!({ "running": false }) } else { status };
                }
                // The window's account: where the plugin's nodes are, and
                // what its row says (the app answers this call itself).
                let tool = if p.snapshot { "@snapshot" } else { "@status" };
                match self
                    .inner
                    .tools
                    .call(&entry.item.id, tool, serde_json::json!({}), None, STATUS_TIMEOUT)
                    .await
                {
                    Ok(window) => {
                        out["open"] = serde_json::json!(true);
                        for (k, v) in window.as_object().into_iter().flatten() {
                            out[k] = v.clone();
                        }
                    }
                    Err(CallError::NotOpen) => {
                        out["open"] = serde_json::json!(false);
                        out["note"] = serde_json::json!("Zeron's window isn't open on this device: open it to see where the plugin shows.");
                    }
                    Err(CallError::TimedOut) => {
                        out["open"] = serde_json::json!(true);
                        out["note"] = serde_json::json!("The window didn't answer in time.");
                    }
                    Err(CallError::Failed(why)) => {
                        out["open"] = serde_json::json!(true);
                        out["note"] = serde_json::json!(why);
                    }
                }
                RpcReply::value(&out)
            }
            methods::DESCRIBE_ELEMENTS => {
                let path = self.inner.plugins_dir.join(ELEMENTS_FILE);
                let text = tokio::fs::read_to_string(&path).await.map_err(|_| {
                    failed("Zeron's window hasn't said where plugins can go yet: open Zeron, then ask again.")
                })?;
                let mut map: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|_| failed("Zeron's map of where plugins can go couldn't be read."))?;
                let (guide, _) = guides::ensure(&self.inner.data_dir);
                map["guide"] = serde_json::json!(guide.display().to_string());
                map["guide_summary"] = serde_json::json!(guides::PLUGINS_SUMMARY);
                RpcReply::value(&map)
            }
            methods::CHECK_PLUGIN_KEY => {
                // Nothing in it is trusted: it only hastens the check.
                let this = self.clone();
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    runtime.spawn(async move { this.check_own_key_soon().await });
                }
                RpcReply::value(&serde_json::json!({ "checking": true }))
            }
            methods::PLUGIN_SIGNING_KEY => {
                // Settings → Account: the account's keys (`keys: true`), and
                // revoking one (`revoke: <key>`; this device's own is also
                // deleted here, and a fresh one made at the next sign-in).
                #[derive(Deserialize, Default)]
                struct KeyParams {
                    #[serde(default)]
                    keys: bool,
                    #[serde(default)]
                    revoke: Option<String>,
                }
                let p: KeyParams = parse_params(params)?;
                if let Some(revoke) = p.revoke.map(|k| k.trim().to_owned()).filter(|k| !k.is_empty()) {
                    let directory = self.directory().ok_or_else(|| failed("Sign in to revoke a key"))?;
                    let own = signing::load_key(&self.inner.data_dir).map(|k| k.public());
                    if own.as_deref() == Some(revoke.as_str()) {
                        self.retire_signing_key().await;
                    } else {
                        // Whose device it's on, to tell it at once.
                        let device = directory
                            .keys_since(0)
                            .await
                            .ok()
                            .and_then(|(rows, _)| rows.into_iter().find(|r| r.key == revoke).map(|r| r.device_id));
                        directory.revoke(&revoke).await.map_err(|_| failed("Couldn't revoke it: the edge can't be reached"))?;
                        if let Some(device) = device.filter(|d| !d.is_empty() && *d != self.inner.device_id) {
                            self.tell_key_revoked(device);
                        }
                    }
                    lock(&self.inner.new_keys).retain(|row| row.key != revoke);
                    // Plugins it signed are checked again: no longer verified.
                    lock(&self.inner.cache).0.remove(&revoke);
                    self.kick(Kick::Rescan);
                    return RpcReply::value(&serde_json::json!({ "revoked": revoke }));
                }
                let listed = if p.keys {
                    let directory = self.directory().ok_or_else(|| failed("Sign in to see your keys"))?;
                    let (rows, _) = directory.keys_since(0).await.map_err(|_| failed("Couldn't list your keys"))?;
                    Some(rows)
                } else {
                    None
                };
                let data_dir = self.inner.data_dir.clone();
                let key =
                    tokio::task::spawn_blocking(move || signing::load_or_create_key(&data_dir))
                        .await
                        .map_err(|_| failed("Couldn't read this device's signing key"))?
                        .map_err(failed)?;
                let registered = self.account().is_some_and(|account| {
                    keys::Registration::load(&self.inner.data_dir).covers(&key.public(), &account)
                });
                // Only the public half, ever: the private key never leaves
                // the engine.
                let new_keys = lock(&self.inner.new_keys).clone();
                let mut reply = serde_json::json!({ "key": key.public(), "registered": registered, "new_keys": new_keys });
                if let Some(rows) = listed {
                    reply["keys"] = serde_json::json!(rows
                        .iter()
                        .map(|row| {
                            let mut row_value = serde_json::json!(row);
                            row_value["this_device"] = serde_json::json!(row.key == key.public());
                            row_value["device_name"] = serde_json::json!(self.device_name(&row.device_id));
                            row_value
                        })
                        .collect::<Vec<_>>());
                }
                RpcReply::value(&reply)
            }
            method if lifecycle::handles(method) => self.lifecycle(method, params).await,
            _ => Err(RpcError::UnknownMethod(method.to_owned())),
        }
    }

    /// `VerifyPlugin`'s reply for any folder.
    async fn verify(&self, dir: &Path) -> serde_json::Value {
        let probe = dir.to_path_buf();
        let root = std::fs::canonicalize(&self.inner.plugins_dir).ok();
        let inspected: Inspected = tokio::task::spawn_blocking(move || {
            let under = std::fs::canonicalize(&probe)
                .ok()
                .and_then(|c| c.parent().map(Path::to_path_buf))
                == root;
            scan::inspect(&probe, under)
        })
        .await
        .unwrap_or_else(|_| scan::inspect(dir, false));
        let own = lock(&self.inner.state).own.contains(&inspected.folder)
            && std::fs::canonicalize(dir).ok()
                == std::fs::canonicalize(self.inner.plugins_dir.join(&inspected.folder)).ok();
        let author = match inspected.signature_author() {
            _ if own => Author::You,
            Ok(author) => author,
            Err(key) => self.resolve_now(&key).await,
        };
        let status = match (&inspected.error, &inspected.sig) {
            (Some(_), _) => "broken",
            (None, SigCheck::Unsigned) => "unsigned",
            (None, SigCheck::FilesDiffer | SigCheck::BadSignature) => "tampered",
            (None, SigCheck::Valid(_)) => "ok",
        };
        let mut reply =
            serde_json::json!({ "author": author, "status": status, "hash": inspected.hash });
        if let Some(error) = inspected.error {
            reply["error"] = serde_json::json!(error);
        }
        reply
    }

    async fn call_tool(&self, p: CallParams) -> Result<RpcReply, RpcError> {
        self.ensure_scanned().await;
        let wanted = match (&p.mcp_name, &p.plugin, &p.tool) {
            (Some(name), _, _) => name.clone(),
            (None, Some(plugin), Some(tool)) => mcp_name(plugin, tool),
            _ => return Err(failed("Say which tool to call")),
        };
        let found = lock(&self.inner.entries)
            .iter()
            .filter(|e| e.allows("tools"))
            .find_map(|e| {
                e.item
                    .tools
                    .iter()
                    .find(|t| mcp_name(&e.item.id, &t.name) == wanted)
                    .map(|t| (e.item.id.clone(), e.item.name.clone(), t.name.clone()))
            });
        let Some((plugin, plugin_name, tool)) = found else {
            return Err(failed(format!("There's no tool {wanted}")));
        };
        let args = if p.args.is_null() {
            serde_json::json!({})
        } else {
            p.args
        };
        // Its service answers it, window or not; a tool it doesn't serve
        // goes to the window.
        if let Some(plan) = self.entry(&plugin).as_ref().and_then(service_plan) {
            let call = serde_json::json!({
                "kind": "tool",
                "from": self.here(),
                "chat": p.chat_id,
            });
            match self
                .inner
                .services
                .call(&plan, service::Kind::Tool, &tool, args.clone(), call, broker::TOOL_TIMEOUT)
                .await
            {
                Ok(value) => {
                    return RpcReply::value(&serde_json::json!({ "ok": true, "value": value }));
                }
                Err(refusal) if refusal.reason == reason::UNKNOWN_METHOD => {}
                Err(refusal) => return Err(failed(refusal.message)),
            }
        }
        match self
            .inner
            .tools
            .call(
                &plugin,
                &tool,
                args,
                p.chat_id.as_deref(),
                broker::TOOL_TIMEOUT,
            )
            .await
        {
            Ok(value) => RpcReply::value(&serde_json::json!({ "ok": true, "value": value })),
            Err(CallError::NotOpen) => Err(failed(format!(
                "Open Zeron on {} to use {tool}",
                self.device_name(&self.inner.device_id)
            ))),
            Err(CallError::TimedOut) => Err(failed(format!("{plugin_name} didn't answer in time"))),
            Err(CallError::Failed(error)) => Err(failed(error)),
        }
    }

    async fn send(&self, p: SendParams) -> Result<RpcReply, RpcError> {
        self.ensure_scanned().await;
        let entry = self
            .entry(&p.plugin)
            .ok_or_else(|| failed(format!("There's no plugin {}", p.plugin)))?;
        if !entry.allows("devices") {
            return Err(failed(format!(
                "{} isn't allowed to reach your other devices",
                entry.item.name
            )));
        }
        if serde_json::to_vec(&p.data)
            .map(|b| b.len())
            .unwrap_or(usize::MAX)
            > MAX_SEND_BYTES
        {
            return Err(failed("That's more than 64 KB to send at once"));
        }
        let targets: Vec<String> = if p.to == "*" {
            let workspace = lock(&self.inner.workspace).clone();
            workspace
                .and_then(|ws| ws.read_devices().ok())
                .unwrap_or_default()
                .into_iter()
                .map(|d| d.id)
                .filter(|id| id != &self.inner.device_id)
                .collect()
        } else {
            vec![p.to.clone()]
        };
        let deliver = serde_json::json!({
            "plugin": p.plugin,
            "from": self.inner.device_id,
            "author_key": author_identity(&entry),
            "data": p.data,
        });
        let links = lock(&self.inner.links).clone();
        let results = futures::future::join_all(targets.into_iter().map(|target| {
            let deliver = deliver.clone();
            let links = links.clone();
            async move {
                if target == self.inner.device_id {
                    let p: DeliverParams = serde_json::from_value(deliver).expect("built above");
                    return (target, self.deliver(p).await.to_owned());
                }
                let Some(links) = links else {
                    return (target, outcome::OFFLINE.to_owned());
                };
                let reply = tokio::time::timeout(DELIVER_TIMEOUT, async {
                    let client = links.client(&target).await?;
                    client.call(methods::PLUGIN_DELIVER, deliver).await
                })
                .await;
                let outcome = match reply {
                    Ok(Ok(reply)) => reply
                        .get("outcome")
                        .and_then(|v| v.as_str())
                        .unwrap_or(outcome::OFFLINE)
                        .to_owned(),
                    // An engine from before plugins.
                    Ok(Err(RpcError::UnknownMethod(_))) => outcome::NOT_INSTALLED.to_owned(),
                    Ok(Err(_)) | Err(_) => {
                        if reply.is_err() {
                            links.invalidate(&target);
                        }
                        outcome::OFFLINE.to_owned()
                    }
                };
                (target, outcome)
            }
        }))
        .await;
        let results: serde_json::Map<String, serde_json::Value> = results
            .into_iter()
            .map(|(id, o)| (id, serde_json::Value::String(o)))
            .collect();
        RpcReply::value(&serde_json::json!({ "results": results }))
    }

    /// The receiving side of `PluginSend` (and of a service's events, sent
    /// to the devices watching it).
    async fn deliver(&self, p: DeliverParams) -> &'static str {
        self.ensure_scanned().await;
        let Some(entry) = self.entry(&p.plugin) else {
            return outcome::NOT_INSTALLED;
        };
        let needs = if p.event.is_some() { "service" } else { "devices" };
        if !entry.allows(needs) {
            return outcome::NOT_ALLOWED;
        }
        // Its size where it lands, whatever the sender checked.
        let size = serde_json::to_vec(&p.data).map_or(usize::MAX, |b| b.len())
            + p.event.as_ref().map_or(0, String::len);
        if size > MAX_SEND_BYTES {
            return outcome::TOO_BIG;
        }
        if !self.same_author(&entry, p.author_key.as_deref()).await {
            return outcome::NOT_ALLOWED;
        }
        let from = self.sender(&p.from, p.unverified_from);
        let mut message = serde_json::json!({
            "plugin": p.plugin,
            "from": from,
            "data": p.data,
            "at": crate::now_ms(),
        });
        if let Some(event) = p.event {
            message["event"] = serde_json::json!(event);
        }
        if self.inner.channel.push(message) {
            outcome::DELIVERED
        } else {
            outcome::NOT_OPEN
        }
    }

    /// Both sides have to be the same author, failing closed: Zeron's own
    /// only with Zeron's own; a signed one only with one signed by the same
    /// key, or by a key of the same account (each looked up now: stale or
    /// offline is Unknown, which matches nothing); unsigned only with
    /// unsigned (the ids, already equal, are all there is).
    async fn same_author(&self, entry: &Entry, there: Option<&str>) -> bool {
        let here = author_identity(entry);
        let (here, there) = match (here.as_deref(), there) {
            (Some(BUILTIN_AUTHOR), there) | (there, Some(BUILTIN_AUTHOR)) => {
                return entry.item.builtin && there == Some(BUILTIN_AUTHOR);
            }
            (None, None) => return true,
            (Some(here), Some(there)) => (here.to_owned(), there.to_owned()),
            // Exactly one side signed.
            _ => return false,
        };
        if here == there {
            return true;
        }
        let (a, b) = futures::join!(self.resolve_now(&here), self.resolve_now(&there));
        same_account(&a, &b)
    }
}

// ── Services ────────────────────────────────────────────────────────────────

/// What `entry`'s service runs as, while it may run here: on, allowed
/// `service`, with a service and a snapshot.
fn service_plan(entry: &Entry) -> Option<Plan> {
    let item = &entry.item;
    let file = item.service.clone()?;
    if !entry.allows("service") {
        return None;
    }
    Some(Plan {
        plugin: item.id.clone(),
        name: item.name.clone(),
        version: item.version.clone(),
        hash: item.hash.clone(),
        dir: PathBuf::from(item.snapshot.as_ref()?),
        entry: file,
        allowed: item.allowed.clone(),
        commands: item.commands.clone(),
        hosts: item.network_hosts.clone(),
    })
}

/// What services borrow from the plugins module.
struct ServiceHooks {
    inner: Weak<Inner>,
}

impl service::Hooks for ServiceHooks {
    fn emit(&self, plugin: &str, event: &str, data: serde_json::Value) {
        if let Some(inner) = self.inner.upgrade() {
            Plugins { inner }.emit_event(plugin, event, data);
        }
    }

    fn notify(&self, plugin: &str, title: &str, body: &str, session: Option<&str>) -> String {
        let notify = self.inner.upgrade().and_then(|inner| lock(&inner.notifier).clone());
        match notify {
            Some(notify) => notify(plugin, title, body, session),
            None => "unavailable".into(),
        }
    }

    fn device(&self) -> serde_json::Value {
        match self.inner.upgrade() {
            Some(inner) => Plugins { inner }.here(),
            None => serde_json::Value::Null,
        }
    }

    fn folders(&self) -> Vec<String> {
        let folders = self.inner.upgrade().and_then(|inner| lock(&inner.folders).clone());
        folders.map(|f| f()).unwrap_or_default()
    }

    /// R11: the snapshot a service loads from is the version allowed, or it
    /// isn't loaded: it's removed, and a rescan makes it again from the
    /// plugin's (checked) folder.
    fn verify(&self, plan: &service::Plan) -> Result<(), String> {
        let Err(why) = snapshot::verify_hash(&plan.dir, &plan.hash) else {
            return Ok(());
        };
        tracing::warn!(plugin = %plan.plugin, %why, "plugins: a service's snapshot changed; not loaded");
        snapshot::remove(&plan.dir);
        if let (Some(inner), Ok(runtime)) = (self.inner.upgrade(), tokio::runtime::Handle::try_current()) {
            runtime.spawn(async move { Plugins { inner }.rescan().await });
        }
        Err(format!("its files changed since you allowed it ({why}); Zeron is checking it again"))
    }
}

impl Plugins {
    /// This device, as `{ id, name }`.
    fn here(&self) -> serde_json::Value {
        let id = &self.inner.device_id;
        serde_json::json!({ "id": id, "name": self.device_name(id) })
    }

    /// Run what should run, after a scan.
    fn sync_services(&self) {
        let plans: Vec<Plan> = lock(&self.inner.entries).iter().filter_map(service_plan).collect();
        self.inner.services.sync(plans);
    }

    /// `zeron.emit`: to this device's windows, and to the devices watching.
    fn emit_event(&self, plugin: &str, event: &str, data: serde_json::Value) {
        self.inner.channel.push(serde_json::json!({
            "plugin": plugin,
            "from": self.here(),
            "event": event,
            "here": true,
            "data": data,
            "at": crate::now_ms(),
        }));
        let now = Instant::now();
        let targets: Vec<String> = {
            let mut watchers = lock(&self.inner.watchers);
            let Some(devices) = watchers.get_mut(plugin) else { return };
            devices.retain(|_, until| *until > now);
            devices.keys().cloned().collect()
        };
        let (Some(links), Ok(runtime)) = (lock(&self.inner.links).clone(), tokio::runtime::Handle::try_current())
        else {
            return;
        };
        let deliver = serde_json::json!({
            "plugin": plugin,
            "from": self.inner.device_id,
            "author_key": self.entry(plugin).and_then(|e| author_identity(&e)),
            "event": event,
            "data": data,
        });
        for target in targets {
            let (links, deliver, this, plugin) = (links.clone(), deliver.clone(), self.clone(), plugin.to_owned());
            runtime.spawn(async move {
                let reply = tokio::time::timeout(DELIVER_TIMEOUT, async {
                    links.client(&target).await?.call(methods::PLUGIN_DELIVER, deliver).await
                })
                .await;
                let gone = match &reply {
                    Ok(Ok(reply)) => matches!(
                        reply.get("outcome").and_then(|o| o.as_str()),
                        Some(outcome::NOT_INSTALLED | outcome::NOT_ALLOWED)
                    ),
                    Ok(Err(RpcError::UnknownMethod(_))) => true,
                    _ => false,
                };
                if gone && let Some(devices) = lock(&this.inner.watchers).get_mut(&plugin) {
                    devices.remove(&target);
                }
            });
        }
    }

    /// `CallPluginService`: a window's call to its service here or on
    /// another device. Always a value: `{ok, value}` or a refusal.
    async fn call_service(&self, p: ServiceParams) -> serde_json::Value {
        self.ensure_scanned().await;
        let here = self.device_name(&self.inner.device_id);
        let Some(entry) = self.entry(&p.plugin) else {
            return Refusal::new(reason::NOT_INSTALLED, format!("There's no plugin {} on {here}", p.plugin)).to_json();
        };
        if p.method.is_empty() {
            return Refusal::new(reason::BAD_CALL, "Say which method to call").to_json();
        }
        let call = ServeParams {
            plugin: p.plugin.clone(),
            version: entry.item.version.clone(),
            author_key: author_identity(&entry),
            from: self.inner.device_id.clone(),
            method: p.method.clone(),
            args: p.args,
            timeout: p.timeout,
            unverified_from: false,
        };
        let target = p.device.map(|d| d.trim().to_owned()).filter(|d| !d.is_empty() && *d != self.inner.device_id);
        let Some(target) = target else {
            return self.serve_call(call).await;
        };
        if p.method == "@watch" {
            lock(&self.inner.watching).insert((p.plugin.clone(), target.clone()));
        }
        self.forward_call(&target, call).await
    }

    /// One call to another device's service, over the device link.
    async fn forward_call(&self, target: &str, call: ServeParams) -> serde_json::Value {
        let name = self.device_name(target);
        let Some(links) = lock(&self.inner.links).clone() else {
            return Refusal::new(reason::OFFLINE, format!("{name} can't be reached: this device isn't connected")).to_json();
        };
        let wait = clamp_timeout(call.timeout) + LINK_SLACK;
        let params = serde_json::to_value(&call).unwrap_or_default();
        let reply = tokio::time::timeout(wait, async {
            links.client(target).await?.call(methods::SERVE_PLUGIN_CALL, params).await
        })
        .await;
        match reply {
            Ok(Ok(value)) => value,
            Ok(Err(RpcError::UnknownMethod(_))) => Refusal::new(
                reason::OLD_ENGINE,
                format!("{name} runs a Zeron from before plugin services: update Zeron there"),
            )
            .to_json(),
            Ok(Err(RpcError::BadParams(why))) => Refusal::new(reason::BAD_CALL, why).to_json(),
            Ok(Err(error)) => {
                if matches!(error, RpcError::Closed | RpcError::Transport(_)) {
                    links.invalidate(target);
                }
                Refusal::new(reason::OFFLINE, format!("{name} can't be reached right now ({error})")).to_json()
            }
            Err(_) => {
                // Perhaps a dead link: the next call dials again.
                links.invalidate(target);
                Refusal::new(reason::TIMEOUT, format!("{name} didn't answer within {} s", wait.as_secs())).to_json()
            }
        }
    }

    /// A call for this device's service (`ServePluginCall`, or one from a
    /// window here): `{ok, value}` or a refusal with its reason.
    async fn serve_call(&self, p: ServeParams) -> serde_json::Value {
        match self.serve_inner(p).await {
            Ok(value) => serde_json::json!({ "ok": true, "value": value }),
            Err(refusal) => refusal.to_json(),
        }
    }

    async fn serve_inner(&self, p: ServeParams) -> Result<serde_json::Value, Refusal> {
        self.ensure_scanned().await;
        let here = self.device_name(&self.inner.device_id);
        let entry = self
            .entry(&p.plugin)
            .ok_or_else(|| Refusal::new(reason::NOT_INSTALLED, format!("{} isn't installed on {here}", p.plugin)))?;
        let name = entry.item.name.clone();
        match entry.item.status {
            Status::On => {}
            Status::Off => return Err(Refusal::new(reason::OFF, format!("{name} is turned off on {here}"))),
            Status::Pending => {
                return Err(Refusal::new(reason::NOT_APPROVED, format!("{name} is waiting to be allowed on {here}")));
            }
            Status::Tampered | Status::Broken => {
                let why = entry.item.error.clone().unwrap_or_else(|| "its files don't match its signature".into());
                return Err(Refusal::new(reason::BROKEN, format!("{name} can't run on {here}: {why}")));
            }
        }
        if entry.item.service.is_none() {
            return Err(Refusal::new(
                reason::NO_SERVICE,
                format!("{name} {} on {here} has no service", entry.item.version),
            ));
        }
        if !entry.allows("service") {
            return Err(Refusal::new(reason::NOT_APPROVED, format!("{name}'s service isn't allowed on {here} yet")));
        }
        // Every caller says its version (an engine before that said none,
        // and was let through).
        if entry.item.version != p.version {
            return Err(Refusal::new(
                reason::VERSION_MISMATCH,
                format!(
                    "{here} has {name} {}, the caller {}: update it so both have the same",
                    entry.item.version, p.version
                ),
            ));
        }
        // From another device: its size checked where it lands.
        if p.from != self.inner.device_id
            && serde_json::to_vec(&p.args).map_or(usize::MAX, |b| b.len()) > MAX_CALL_BYTES
        {
            return Err(Refusal::new(reason::BAD_CALL, "That call's arguments are too big"));
        }
        if !self.same_author(&entry, p.author_key.as_deref()).await {
            return Err(Refusal::new(
                reason::OTHER_AUTHOR,
                format!("The {name} on {here} is someone else's plugin by the same name"),
            ));
        }
        let plan = service_plan(&entry)
            .ok_or_else(|| Refusal::new(reason::NOT_LOADED, format!("{name} isn't ready on {here} yet")))?;
        if p.method == "@watch" {
            // Only another of your devices is sent events (never an id
            // that isn't one: `trusted_from` empties it).
            if !p.from.is_empty() && p.from != self.inner.device_id {
                lock(&self.inner.watchers)
                    .entry(p.plugin.clone())
                    .or_default()
                    .insert(p.from.clone(), Instant::now() + WATCH_FOR);
            }
            return Ok(serde_json::Value::Null);
        }
        if p.method.is_empty() || p.method.starts_with('@') {
            return Err(Refusal::new(reason::BAD_CALL, format!("\"{}\" isn't a method", p.method)));
        }
        let from = if p.from.is_empty() && !p.unverified_from {
            self.sender(&self.inner.device_id, false)
        } else {
            self.sender(&p.from, p.unverified_from)
        };
        let call = serde_json::json!({
            "kind": "call",
            "from": from,
        });
        let args = if p.args.is_null() { serde_json::json!({}) } else { p.args };
        self.inner
            .services
            .call(&plan, service::Kind::Method, &p.method, args, call, clamp_timeout(p.timeout))
            .await
    }
}

/// A call's timeout: what it asks (seconds), within the service's bounds.
fn clamp_timeout(asked: Option<f64>) -> Duration {
    asked
        .filter(|s| s.is_finite() && *s > 0.0)
        .map_or(service::DEFAULT_TIMEOUT, |s| Duration::from_secs_f64(s).min(service::MAX_TIMEOUT))
}

/// Whether two resolved authors are the same account: this device's key
/// and the account's other devices' keys are; two verified keys are when
/// they have the same username. Unknown matches nothing.
pub fn same_account(a: &Author, b: &Author) -> bool {
    let mine = |x: &Author| matches!(x, Author::You | Author::Verified { mine: true, .. });
    match (a, b) {
        _ if mine(a) && mine(b) => true,
        (Author::Verified { username: Some(x), .. }, Author::Verified { username: Some(y), .. }) => x == y,
        _ => false,
    }
}

/// What a device that isn't vouched for is called (`from`).
pub const UNKNOWN_DEVICE: &str = "unknown";

/// Who a plugin's author is across devices: Zeron for its own, else its
/// signing key (none when unsigned).
fn author_identity(entry: &Entry) -> Option<String> {
    if entry.item.builtin {
        return Some(BUILTIN_AUTHOR.to_owned());
    }
    entry.key().map(str::to_owned)
}

impl Plugins {
    /// `from` as the request's caller vouches for it: this device's own app
    /// says who it is; another device is the one the edge verified on the
    /// link (`Caller::Peer{device}`). Anything else (a dial the edge didn't
    /// verify, an agent) is unknown: empty, and never sent events.
    fn trusted_from(&self, claimed: String, unverified: &mut bool) -> String {
        match zeron_rpc::caller() {
            zeron_rpc::Caller::Local => claimed,
            zeron_rpc::Caller::Peer { device: Some(device) } => device,
            zeron_rpc::Caller::Peer { device: None } | zeron_rpc::Caller::Agent { .. } => {
                *unverified = true;
                String::new()
            }
        }
    }

    /// `{id, name}` of who sent it, as vouched for (`trusted_from`).
    fn sender(&self, from: &str, unverified: bool) -> serde_json::Value {
        if from.is_empty() && unverified {
            return serde_json::json!({ "id": UNKNOWN_DEVICE, "name": "Another device", "unverified": true });
        }
        let mut sender = serde_json::json!({ "id": from, "name": self.device_name(from) });
        if unverified {
            sender["unverified"] = serde_json::json!(true);
        }
        sender
    }
}

/// A watch receiver as a stream: the current value at once, then each change.
fn watch_stream(rx: watch::Receiver<serde_json::Value>) -> BoxStream<'static, serde_json::Value> {
    futures::stream::unfold((rx, false), |(mut rx, emitted)| async move {
        if emitted {
            rx.changed().await.ok()?;
        }
        let value = rx.borrow_and_update().clone();
        Some((value, (rx, true)))
    })
    .boxed()
}

#[cfg(test)]
mod authority_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod identity_tests;
#[cfg(test)]
mod service_tests;
#[cfg(all(test, unix))]
mod git_service_tests;
