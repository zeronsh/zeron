//! Plugins in the main window. The engine decides what loads: it reads the
//! plugins folder, checks each plugin's signature, keeps what you allowed,
//! and copies each plugin it may run into a snapshot that never changes
//! (`WatchPlugins`). This side only shows it: the plugins it says are on
//! load from their snapshots into zgx (`set_plugins`, each with the grants
//! its permissions map to, `grants`), so nothing edited runs before the
//! engine has checked it again. A change reloads the window's scripts,
//! keeping their state.
//!
//! The window itself runs your active template (`live`): Zeron's own
//! interface with the template's files on top, in `<data>/templates/.live/`,
//! rewritten as you switch or save and reloaded in place.
//!
//! What plugins reach of Zeron, besides the hooks and screens zgx gives
//! them (`grants::HOOKS`): read-only shared state the app keeps
//! (`grants::SHARED_READ`, screens/plugins), the app's actions
//! (`grants::ACTIONS`), and their own calls and actions, `plugin.<id>.*`:
//!
//! - `plugin.<id>.calls` (shared, read only): what's waiting for the
//!   plugin, an agent's tool calls (from the engine's `ServePluginTools`)
//!   and menu items chosen; the plugin answers each with
//!   `plugin.<id>.result { id, ok, value?, error? }`.
//! - `plugin.<id>.inbox` (shared, read only): what its copies on your
//!   other devices sent (`WatchPluginChannel`), `{ seq, from, data, at }`,
//!   until it says it heard them (`plugin.<id>.ack(seq)`);
//!   `plugin.<id>.send { to, data }` sends (`PluginSend`).
//! - `plugin.<id>.menus(items)`: its menu items; `plugin.<id>.toast(text)`.
//!
//! Allowing a plugin happens in a window of its own (`consent`).

pub mod consent;
pub mod elements;
#[cfg(target_os = "macos")]
pub mod file_events;
#[cfg(windows)]
#[path = "file_events_windows.rs"]
pub mod file_events;
pub mod grants;
pub mod live;
pub mod records;
pub mod surfaces;
pub mod github;
pub mod store;
pub mod commands;
pub mod db;
pub mod settings;
pub mod recovery;
pub mod remote;
pub mod templates;
pub mod retired_themes;
pub mod official;
pub mod tokens;
pub mod looks;
pub mod template_consent;
pub mod preview;
pub mod host_bar;
pub mod locales;
pub mod network;
pub mod plugin_menus;

use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

use serde_json::{Value as Json, json};
use zgx_desktop::{TaskSpawner, WindowContext, WindowFactory};
use zgx_script::{Capability, Plugin, ScriptHost, Shared, Value};

use crate::auth::text;
use crate::engine::{
    self,
    rpc::{Rpc, Watch},
};

/// How often the templates folder is looked at for a change, where the
/// kernel doesn't say (not macOS or Windows, or its watch couldn't be made). A look
/// that finds nothing changed is a stat of each file, off the UI thread,
/// and composes nothing.
const TEMPLATES_LOOK: Duration = Duration::from_secs(2);
/// How long the main window runs before its launch counts as one that
/// didn't crash.
const LAUNCH_SETTLES: Duration = Duration::from_secs(20);
/// The messages a plugin's inbox holds before the oldest go.
const INBOX: usize = 256;
/// How many of the requests to the app are kept (the app has run every
/// earlier one).
const REQUESTS: usize = 32;
/// The most a menu item's label says.
const LABEL: usize = 60;
/// Plugin ids that are Zeron's own.
const RESERVED: &[&str] = &["templates", "zeron", "app"];
/// How many of a template's files may fall back in a run before the whole
/// template is set aside (it isn't converging: its root depends on them).
const MOST_FALLBACKS: usize = 12;

pub(crate) fn json_value(json: &Json) -> Value {
    Value::from_json(&json.to_string()).unwrap_or(Value::Nil)
}

pub(crate) fn value_json(value: &Value) -> Json {
    serde_json::from_str(&value.to_json()).unwrap_or(Json::Null)
}

/// A plugin as the engine lists it (the parts this side reads).
#[derive(Clone, Debug, Default)]
pub struct Item {
    pub json: Json,
}

impl Item {
    fn str(&self, key: &str) -> &str {
        self.json[key].as_str().unwrap_or("")
    }
    pub fn id(&self) -> &str {
        self.str("id")
    }
    pub fn name(&self) -> &str {
        let name = self.str("name");
        if name.is_empty() { self.id() } else { name }
    }
    pub fn status(&self) -> &str {
        self.str("status")
    }
    pub fn hash(&self) -> &str {
        self.str("hash")
    }
    fn list(&self, at: &Json) -> Vec<String> {
        at.as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    }
    pub fn permissions(&self) -> Vec<String> {
        self.list(&self.json["permissions"])
    }
    /// The hosts its requests may reach, as its manifest names them (none:
    /// any public address). What the window's requests are held to
    /// (`network`), and the engine's for its service.
    pub fn network_hosts(&self) -> Vec<String> {
        self.list(&self.json["network_hosts"])
    }
    /// Its "ui.change" targets, as its manifest lists them.
    pub fn change(&self) -> Vec<String> {
        self.list(&self.json["ui"]["change"])
    }
    /// Whether it runs, with `permission` among what it was allowed.
    pub fn may(&self, permission: &str) -> bool {
        self.status() == "on" && self.permissions().iter().any(|p| p == permission)
    }
    /// What it loads as, if it runs: its snapshot's entry and its grants.
    pub fn plugin(&self) -> Option<Plugin> {
        if self.status() != "on" || RESERVED.contains(&self.id()) {
            return None;
        }
        let snapshot = PathBuf::from(self.json["snapshot"].as_str()?);
        let entry = self.json["entry"].as_str().filter(|e| !e.is_empty()).unwrap_or("init.luau");
        if entry.contains("..") || entry.starts_with('/') {
            return None;
        }
        let ui = &self.json["ui"];
        Some(Plugin {
            name: self.id().into(),
            entry: snapshot.join(entry),
            grants: grants::capabilities(
                self.id(),
                &self.permissions(),
                &self.list(&ui["change"]),
                &self.list(&ui["replace"]),
            ),
        })
    }
}

#[derive(Default)]
struct Calls {
    shared: Option<Shared>,
    /// Waiting for the plugin, by id: each call's kind, and what to say back.
    list: Vec<(String, Value)>,
}

#[derive(Default)]
struct Inbox {
    shared: Option<Shared>,
    list: Vec<Value>,
    seq: u64,
}

/// A plugin's storage: its `plugin.<id>.store`, and what it holds.
type Store = (Shared, serde_json::Map<String, Json>);

/// A plugin's GitHub: its `plugin.<id>.github` and the watches behind it,
/// by session.
#[derive(Default)]
struct Gh {
    shared: Option<Shared>,
    prs: serde_json::Map<String, Json>,
    watches: github::Watches<Watch>,
}

pub struct Bridge {
    host: ScriptHost,
    /// Where each plugin's requests may go, as its manifest names them.
    network: network::Network,
    windows: WindowFactory,
    page: Shared,
    menus: Shared,
    requests: Shared,
    request_list: RefCell<Vec<Value>>,
    request_seq: Cell<u64>,
    state: Cell<&'static str>,
    items: RefCell<Vec<Item>>,
    dirs: RefCell<Option<(PathBuf, PathBuf)>>,
    /// What zgx loads now (plugins, then the templates' own).
    loaded: RefCell<Vec<Plugin>>,
    /// The ids whose `plugin.<id>.*` are registered.
    exposed: RefCell<HashSet<String>>,
    calls: RefCell<HashMap<String, Calls>>,
    inbox: RefCell<HashMap<String, Inbox>>,
    item_menus: RefCell<BTreeMap<String, Vec<Value>>>,
    toasted: RefCell<HashMap<String, Instant>>,
    /// What was said once this run: a signing key's notice (`key-notice:<id>`),
    /// and a template package whose window opened as it arrived
    /// (`template:<name>`: no notice for it as well).
    told: RefCell<HashSet<String>>,
    /// `<data>/templates`, as the engine's data dir says (from its config).
    templates_root: Option<PathBuf>,
    /// What `.live` holds now.
    overlay: RefCell<live::Overlay>,
    /// What `.live` was laid down from: a look that finds the same composes
    /// nothing.
    laid: RefCell<live::Laid>,
    /// A look is on its way (its file work off this thread); another asked
    /// meanwhile runs once it's done.
    looking: Cell<bool>,
    look_again: Cell<bool>,
    /// The app itself switched templates (its Templates page, the host's
    /// menu): the next switch a look finds is yours, and isn't noticed.
    chose: Cell<bool>,
    /// Whether the active template is trusted (preview.rs), as of its id,
    /// its files' stamp and the approvals file's.
    trust: RefCell<Option<((String, String, String), bool)>>,
    /// A template's files that failed (`<id>:<rel>`, by the file as it stood),
    /// Zeron's showing in their place until they change.
    failed_files: RefCell<recovery::Failures>,
    /// Templates set aside whole (by id, as their files were), until edited.
    broken: RefCell<recovery::Failures>,
    /// How many files fell back, by template, this run.
    fallbacks: RefCell<HashMap<String, usize>>,
    /// The templates page's data (screens/templates).
    templates_page: Shared,
    /// Plugins that failed, kept out until they change.
    failed_plugins: RefCell<recovery::Failures>,
    ui: zgx::widgets::WeakUi,
    /// Templates' theme files, read once a change (their previews).
    looks: looks::Cache,
    /// Each plugin's records (`records`), and what keeps them up to date.
    records: RefCell<HashMap<String, Shared>>,
    records_effect: RefCell<Option<zgx::reactive::Effect>>,
    /// What each plugin's nodes couldn't do, by plugin: as of which version,
    /// and in words ("Focus timer needs a session, but it's in the title bar").
    problems: RefCell<HashMap<String, (String, String)>>,
    /// Each plugin's pages (`add_page`), in the order added, and the list
    /// the sidebar draws (`plugins.pages`).
    pages: RefCell<BTreeMap<String, Vec<Json>>>,
    pages_shared: Shared,
    /// The main window's view, for zgx's report of what's in it.
    view: RefCell<Option<zgx::compose::ViewHandle>>,
    /// Each plugin's GitHub ("github", lent by proxy): what it hears of the
    /// sessions it shows, and the engine's watches of them (`github`).
    gh: RefCell<HashMap<String, Gh>>,
    /// Each plugin's storage (`zeron.store`): its `plugin.<id>.store` and
    /// what it holds, kept in `<data>/plugins/.storage/<id>.json`.
    stores: RefCell<HashMap<String, Store>>,
    /// Each plugin's settings (`settings`): what it declares and what you
    /// set; the list Settings draws a page of each from
    /// (`plugins.settings`, the app's alone), and each plugin's values
    /// (`plugin.<id>.settings`, its own, read only).
    settings: RefCell<settings::Book>,
    settings_list: Shared,
    settings_own: RefCell<HashMap<String, Shared>>,
    /// The main window's dialogs (a folder setting's).
    dialogs: zgx_desktop::FileDialogs,
    /// What building each plugin's nodes costs, as its SDK last said.
    costs: RefCell<HashMap<String, Json>>,
    /// Has the templates folder looked at (and watched) again, as it moves.
    rewatch: RefCell<Option<zgx::task::LatestSender<()>>>,
    tasks: TaskSpawner,
    watches: RefCell<Vec<crate::engine::rpc::SetUpWatch>>,
    pub(crate) consent: RefCell<Option<consent::Consent>>,
    reviewed: Cell<bool>,
    /// Another device's plugins, shown by Settings → Plugins' switcher
    /// (`plugins.remote`; never loaded here).
    pub(crate) remote: remote::Remote,
    me: RefCell<Weak<Bridge>>,
}

/// What plugins may read and ask of the app, each behind its permission,
/// and every declared hook behind "ui.add".
pub fn expose_surface(host: &ScriptHost) {
    // Names under `zeron` are the app's alone, whatever a plugin is called
    // (its slots and commands are its own names anyway).
    host.reserve_prefix("zeron");
    // Adding before or after any element, by any selector, is "ui.add"'s
    // (replacing is "ui.change"'s, an OverrideElement of the selector as
    // written).
    host.require_contribution_capability(Capability::Named(grants::UI_ADD.into()));
    // What a file or a reply's block is, as an element gives it: a token
    // (`tokens`), never the path or the text.
    tokens::install(host);
    // Slots whose place gives what only "files.read" reads (Git's: a
    // checkout's folder, its remote, commits' authors).
    for slot in grants::FILES_SLOTS {
        host.expose_hook(format!("slot:{slot}"), Some(Capability::Named(grants::UI_ADD_FILES.into())));
    }
    for (key, capability) in grants::SHARED_READ {
        host.expose_shared_read(*key, capability.map(|c| Capability::Named(c.into())));
    }
    for (key, capability) in grants::SHARED_WRITE {
        host.expose_shared(*key, Some(Capability::Named((*capability).into())));
    }
    for (name, capability) in grants::ACTIONS {
        host.expose_action(*name, Some(Capability::Named((*capability).into())));
    }
}

/// Plugin `id`'s own calls, messages and actions, open to it alone.
pub fn expose_own(host: &ScriptHost, id: &str) {
    let own = grants::own(id);
    for name in grants::OWN_SHARED {
        host.expose_shared_read(format!("plugin.{id}.{name}"), Some(own.clone()));
    }
    for name in grants::OWN_ACTIONS {
        host.expose_action(format!("plugin.{id}.{name}"), Some(own.clone()));
    }
    // What's behind a token an element gave it, as its records allow.
    tokens::serve(host, id, &host.shared(&format!("plugin.{id}.records"), Value::Nil));
}

/// The plugin watches on connection `rpc`. The engine answers them (and
/// every plugin call) `setup_incomplete` while the account's setup isn't
/// finished (no organization chosen): then they're asked again once it is
/// (`auth::set_up`), on the same connection, so plugins load as setup
/// finishes, with no relaunch. Settings → Plugins says so meanwhile.
fn connect(b: &Rc<Bridge>, rpc: &Rpc) {
    // Zeron's own plugins, shipped beside the app's scripts: the engine
    // scans them with yours (each connection: an engine started again
    // forgets).
    let builtin = builtin_dir();
    if builtin.is_dir() {
        b.tasks.spawn(async move {
            let asked = crate::auth::after_setup(|| {
                let builtin = builtin.clone();
                async move { call("SetBuiltinPlugins", json!({ "dir": builtin })).await }
            });
            if let Err(error) = asked.await {
                eprintln!("zeron-zgx: Zeron's own plugins weren't loaded: {error}");
            }
        });
    }
    // Asked again once setup is finished (`Rpc::watch_set_up`), the page
    // saying so meanwhile (`wait_for_setup`).
    let wait = |b: &Rc<Bridge>| {
        let me = Rc::downgrade(b);
        move |again: crate::engine::rpc::Again| {
            if let Some(b) = me.upgrade() {
                b.wait_for_setup(again);
            }
        }
    };
    let mut watches = Vec::new();
    let me = Rc::downgrade(b);
    watches.push(rpc.watch_set_up(
        "WatchPlugins",
        json!({}),
        move |item| {
            if let Some(b) = me.upgrade() {
                b.heard(&item);
            }
        },
        wait(b),
    ));
    let me = Rc::downgrade(b);
    watches.push(rpc.watch_set_up(
        "ServePluginTools",
        json!({}),
        move |call| {
            if let Some(b) = me.upgrade() {
                b.tool_call(&call);
            }
        },
        wait(b),
    ));
    let me = Rc::downgrade(b);
    watches.push(rpc.watch_set_up(
        "WatchPluginChannel",
        json!({}),
        move |message| {
            if let Some(b) = me.upgrade() {
                b.message(&message);
            }
        },
        wait(b),
    ));
    *b.watches.borrow_mut() = watches;
    // Another device's, if Settings → Plugins shows one.
    remote::load(b);
}

/// The plugins in `before` gone from `now`: removed, or there again from
/// somewhere else (another source) under the same id. An update from where
/// it came keeps what it had.
fn gone(before: &[Item], now: &[Item]) -> Vec<(String, Item)> {
    let origin = |i: &Item| (i.json["source"]["kind"].clone(), i.json["source"]["url"].clone(), i.json["source"]["path"].clone());
    before
        .iter()
        .filter(|old| !now.iter().any(|new| new.id() == old.id() && origin(new) == origin(old)))
        .map(|old| (old.id().to_string(), old.clone()))
        .collect()
}

/// The most of a review comment's code that goes with it, as it's sent
/// (screens/chat/comments `SNIPPET_LINES`, `SNIPPET_BYTES`), and of its note.
const COMMENT_LINES: usize = 40;
const COMMENT_BYTES: usize = 4000;
const COMMENT_NOTE: usize = 4000;

/// A plugin's review comment as it may go on a draft: on the chat shown
/// (`shown`, its id) alone, never another's kept draft; its code at most
/// what's sent (40 lines), its note bounded. None: not on the chat shown.
pub(crate) fn comment(asked: &Value, shown: &str) -> Option<Value> {
    let session = asked.field("session").map(text).filter(|s| !s.is_empty());
    if shown.is_empty() || session.as_deref().is_some_and(|s| s != shown) {
        return None;
    }
    let mut out = value_json(asked);
    let cut = |text: &str, bytes: usize| -> String {
        let mut end = text.len().min(bytes);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].to_string()
    };
    if let Some(snippet) = out["snippet"].as_str() {
        let lines: Vec<&str> = snippet.split('\n').take(COMMENT_LINES).collect();
        out["snippet"] = json!(cut(&lines.join("\n"), COMMENT_BYTES));
    }
    if let Some(note) = out["note"].as_str() {
        out["note"] = json!(cut(note, COMMENT_NOTE));
    }
    out["session"] = json!(shown);
    Some(json_value(&out))
}

/// Plugins for the main window `cx` shows `host` in.
pub fn install(cx: &mut WindowContext, host: &ScriptHost) {
    expose_surface(host);
    let plugin_network = network::Network::default();
    host.fetch_policy(plugin_network.clone());
    // What the app's VM (a template in preview, while it is one) may reach.
    host.guard(preview::guard);
    let bridge = Rc::new(Bridge {
        host: host.clone(),
        network: plugin_network,
        windows: cx.windows.clone(),
        page: host.shared("plugins", page_value("loading", None, &[], &[])),
        menus: host.shared("plugins.menus", vlist(Vec::new())),
        requests: host.shared("plugins.request", vlist(Vec::new())),
        request_list: RefCell::default(),
        request_seq: Cell::new(0),
        state: Cell::new("loading"),
        items: RefCell::default(),
        dirs: RefCell::default(),
        loaded: RefCell::default(),
        exposed: RefCell::default(),
        calls: RefCell::default(),
        inbox: RefCell::default(),
        item_menus: RefCell::default(),
        toasted: RefCell::default(),
        told: RefCell::default(),
        templates_root: templates_root(),
        overlay: RefCell::new(live_overlay()),
        laid: RefCell::new(LIVE_LAID.with(|l| l.borrow().clone())),
        looking: Cell::new(false),
        look_again: Cell::new(false),
        chose: Cell::new(false),
        trust: RefCell::default(),
        failed_files: RefCell::default(),
        broken: RefCell::default(),
        fallbacks: RefCell::default(),
        templates_page: host.shared("templates", Value::record([("state", Value::from("loading"))])),
        failed_plugins: RefCell::default(),
        ui: cx.ui.downgrade(),
        looks: looks::Cache::default(),
        records: RefCell::default(),
        records_effect: RefCell::default(),
        problems: RefCell::default(),
        pages: RefCell::default(),
        pages_shared: host.shared("plugins.pages", vlist(Vec::new())),
        view: RefCell::default(),
        gh: RefCell::default(),
        stores: RefCell::default(),
        settings: RefCell::new(settings::Book::new(settings::storage_dir())),
        settings_list: host.shared("plugins.settings", vlist(Vec::new())),
        settings_own: RefCell::default(),
        dialogs: cx.dialogs.clone(),
        costs: RefCell::default(),
        rewatch: RefCell::default(),
        tasks: cx.tasks.clone(),
        watches: RefCell::default(),
        consent: RefCell::default(),
        reviewed: Cell::new(false),
        remote: remote::Remote::new(host.shared("plugins.remote", remote::Remote::initial())),
        me: RefCell::default(),
    });
    *bridge.me.borrow_mut() = Rc::downgrade(&bridge);
    BRIDGE.with(|b| *b.borrow_mut() = Rc::downgrade(&bridge));
    bridge.actions();

    // Each plugin's records, as what they're made of changes.
    let weak = Rc::downgrade(&bridge);
    let effect = cx.ui.runtime.effect(move || {
        if let Some(b) = weak.upgrade() {
            let (sources, list) = b.sources(true);
            b.publish_records(&sources, &list);
            b.github_wake(&list);
        }
    });
    *bridge.records_effect.borrow_mut() = Some(effect);

    // Every script error, blamed on a template or a plugin when it's
    // theirs: that one makes way for Zeron's own (after this turn, never
    // reloading from inside the error's report).
    let weak = Rc::downgrade(&bridge);
    host.on_error(move |error| {
        if let Some(b) = weak.upgrade() {
            b.later_blame(error.to_string());
        }
    });

    let weak = Rc::downgrade(&bridge);
    engine::get().on_connect(move |rpc| {
        let Some(b) = weak.upgrade() else { return };
        connect(&b, rpc);
    });

    // Your templates, looked at as they change: as the kernel says they
    // did (macOS's FSEvents, Windows' directory change notifications), so
    // an idle window doesn't wake for them; elsewhere, looked at every
    // `TEMPLATES_LOOK`. (On Windows the timed look cost about 6% of a core
    // at idle: each look's `publish_page` → `elements_report` walks, reads
    // and scans every script on the UI thread.) Where Windows may not hear a
    // change (a folder it can't read, a reader that stopped), it looks on the
    // timer too until it can.
    let weak = Rc::downgrade(&bridge);
    #[cfg(any(target_os = "macos", windows))]
    let events = file_events::FileEvents::new();
    #[cfg(any(target_os = "macos", windows))]
    if let Some(events) = &events {
        *bridge.rewatch.borrow_mut() = Some(events.poke());
    }
    cx.tasks.spawn(async move {
        #[cfg(any(target_os = "macos", windows))]
        if let Some(mut events) = events {
            loop {
                {
                    let Some(b) = weak.upgrade() else { return };
                    // Watched first, then looked at: a change between the
                    // two is heard, and looked at again.
                    let (files, dirs) = b.templates_dir().map(|d| live::watched(&d, &crate::ui_dir())).unwrap_or_default();
                    events.watch(files, dirs);
                    b.look();
                }
                #[cfg(windows)]
                let more = events.changed_or_every(TEMPLATES_LOOK).await;
                #[cfg(target_os = "macos")]
                let more = events.changed().await;
                if !more {
                    return;
                }
            }
        }
        loop {
            zgx::timer::sleep(TEMPLATES_LOOK).await;
            let Some(b) = weak.upgrade() else { return };
            b.look();
        }
    });
    // The window has run a while with its template: not a launch that
    // crashed (two of those in a row start safe mode, preview.rs).
    cx.tasks.spawn(async {
        zgx::timer::sleep(LAUNCH_SETTLES).await;
        preview::launch_settled();
    });
    let keep = RefCell::new(Some(bridge));
    cx.on_closed(move || {
        if let Some(b) = keep.borrow_mut().take() {
            b.watches.borrow_mut().clear();
            if let Some(c) = b.consent.borrow_mut().take() {
                c.close();
            }
        }
    });
}

/// The map of where plugins can go, for `describe_elements`, written into
/// `<data>/plugins/.elements.json` only when it changes (the engine watches
/// that folder).
fn write_elements(map: &Json) {
    let Some(dir) = engine::data_dir().map(|d| d.join("plugins")) else { return };
    let Ok(text) = serde_json::to_string_pretty(map) else { return };
    let path = dir.join(".elements.json");
    if std::fs::read_to_string(&path).is_ok_and(|was| was == text) {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    let aside = dir.join(".elements.json.new");
    if std::fs::write(&aside, &text).is_ok() {
        let _ = std::fs::rename(&aside, &path);
    }
}

/// `<data>/templates`, the engine's data dir's (from its config): where
/// templates live, and `.live`, what the main window runs.
pub fn templates_root() -> Option<PathBuf> {
    engine::data_dir().map(|dir| dir.join("templates"))
}

fn live_overlay() -> live::Overlay {
    LIVE_OVERLAY.with(|o| o.borrow().clone())
}

thread_local! {
    /// What `prepare_live` laid down, for the bridge to start from.
    static LIVE_OVERLAY: RefCell<live::Overlay> = RefCell::default();
    /// And what it laid it down from, so the window's first look composes
    /// nothing again.
    static LIVE_LAID: RefCell<live::Laid> = RefCell::default();
}

/// `work` on a thread of its own, its result once done (none if it panicked):
/// a look's file work, which reads folders of a thousand files.
pub(crate) async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tell, mut heard) = zgx::task::latest::<T>();
    std::thread::spawn(move || {
        tell.send(work());
    });
    heard.recv().await
}

/// The main window's app file: `.live/app.luau`, laid down now from Zeron's
/// own and the active template (before the window's host is made, which
/// fixes its file), or `ui/app.luau` where there's no templates folder.
/// The `--zeron-template` launch flag makes Zeron's own the active one first.
pub fn prepare_live() -> PathBuf {
    let fallback = crate::ui_dir().join("app.luau");
    let Some(root) = templates_root() else { return fallback };
    // The per-screen templates' copies (Oct 31, development only): gone.
    let _ = std::fs::remove_dir_all(root.join(".loaded"));
    // Safe mode (preview.rs): as Zeron last closed, ⌥ held now, or a
    // template that crashed the last two launches. `--zeron-template` is
    // the same, asked on the command line.
    preview::read_at_launch();
    if ZERON_TEMPLATE.swap(false, std::sync::atomic::Ordering::Relaxed) {
        preview::enter_safe();
    }
    {
        let id = live::active(&root);
        preview::launch_started(id != live::BUILTIN);
        preview::set_template_running(id != live::BUILTIN);
        preview::set((!preview::trusted(&root, &id)).then(|| preview::Preview { name: template_name(&root, &id), id: id.clone() }));
    }
    if let Err(error) = live::sync_zeron(&crate::ui_dir(), &root) {
        eprintln!("zeron-zgx: Zeron's own interface wasn't copied ({error}); running it from ui/");
        return fallback;
    }
    // The built-in themes once installed as templates, retired: one in use
    // gives way to the default template, wearing that theme
    // (retired_themes.rs).
    if let Some(data) = crate::engine::data_dir() {
        retired_themes::retire(&root, &data);
    }
    let id = live::active(&root);
    match live::compose(&root, &id, &Default::default(), false) {
        Ok((_, overlay)) => {
            LIVE_OVERLAY.with(|o| *o.borrow_mut() = overlay);
            let template_stamp = if id == live::BUILTIN { String::new() } else { live::template_stamp(&root.join(&id)) };
            let laid = live::Laid { ui_stamp: live::ui_stamp(&crate::ui_dir()), id, template_stamp, ..Default::default() };
            LIVE_LAID.with(|l| *l.borrow_mut() = laid);
            // (Pictures of templates taken from the window, before their
            // previews were drawn from their colours: gone.)
            let _ = std::fs::remove_dir_all(root.join(".previews"));
            let _ = std::fs::remove_dir_all(root.join(live::LIVE).join("previews"));
            root.join(live::LIVE).join("app.luau")
        }
        Err(error) => {
            eprintln!("zeron-zgx: the template wasn't laid down ({error}); running Zeron's own from ui/");
            fallback
        }
    }
}

/// The host's own menu item's action ("Use Zeron v3").
pub const USE_ZERON_TEMPLATE: &str = "zeron.use-zeron-template";
/// The host's own "Keep <template>…": the keep sheet for the one in preview.
pub const KEEP_TEMPLATE: &str = "zeron.keep-template";

/// One of the host's own commands (its menu items and its toasts' buttons:
/// never a script's, `zeron.` being the host's).
pub fn host_command(id: &str) {
    // (A test hears what ran instead: Revert's safe mode would be written
    // to the app's own data folder.)
    #[cfg(test)]
    {
        HOST_COMMANDS_RAN.with(|r| r.borrow_mut().push(id.to_owned()));
        return;
    }
    #[allow(unreachable_code)]
    match id {
        USE_ZERON_TEMPLATE => use_zeron_template(),
        KEEP_TEMPLATE => {
            if let (Some(p), Some(b)) = (preview::current(), BRIDGE.with(|b| b.borrow().upgrade())) {
                template_consent::keep(&b, &p.id);
            }
        }
        other => eprintln!("zeron-zgx: no host command {other}"),
    }
}

#[cfg(test)]
thread_local! {
    /// The host commands a test ran (`host_command`).
    pub(crate) static HOST_COMMANDS_RAN: RefCell<Vec<String>> = RefCell::default();
}

/// `work` on the main window's tasks (the host's toasts' timers).
pub(crate) fn spawn_on_bridge(work: impl std::future::Future<Output = ()> + 'static) {
    if let Some(b) = BRIDGE.with(|b| b.borrow().upgrade()) {
        b.tasks.spawn(work);
    }
}

static ZERON_TEMPLATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `--zeron-template`: Zeron's own as the main window opens.
pub fn ask_zeron_template() {
    ZERON_TEMPLATE.store(true, std::sync::atomic::Ordering::Relaxed);
}

thread_local! {
    /// The main window's bridge, for what the app's host tells it.
    static BRIDGE: RefCell<Weak<Bridge>> = const { RefCell::new(Weak::new()) };
    /// Plugin commands the menu bar shows without their shortcut, by
    /// plugin (src/menu_bar.rs).
    static SHORTCUTS_TAKEN: RefCell<BTreeMap<String, Vec<crate::menu_bar::Dropped>>> = RefCell::default();
}

/// The menu bar's account of plugin commands shown without their shortcut
/// (src/menu_bar.rs `Bar::of`): each says so on its plugin's row.
pub fn shortcuts_taken(list: Vec<crate::menu_bar::Dropped>) {
    let mut by: BTreeMap<String, Vec<crate::menu_bar::Dropped>> = BTreeMap::new();
    for dropped in list {
        by.entry(dropped.plugin.clone()).or_default().push(dropped);
    }
    let changed = SHORTCUTS_TAKEN.with(|t| {
        let same = *t.borrow() == by;
        if !same {
            *t.borrow_mut() = by;
        }
        !same
    });
    if !changed {
        return;
    }
    // (After this turn: told from the menu bar's effect.)
    if let Some(b) = BRIDGE.with(|b| b.borrow().upgrade()) {
        let me = Rc::downgrade(&b);
        b.tasks.spawn(async move {
            if let Some(b) = me.upgrade() {
                b.publish_page();
            }
        });
    }
}

/// What a plugin's row says of its commands shown without their shortcut
/// (`lost`, at least one), naming another plugin by `name_of` its id: "Its
/// ⌘J for “Open board” is already Zeron's (Toggle terminal), so it's in the
/// menu bar without one."
pub(crate) fn shortcut_notice(lost: &[crate::menu_bar::Dropped], name_of: impl Fn(&str) -> String) -> String {
    use crate::menu_bar::Why;
    let first = &lost[0];
    let key = shortcut_words(&first.key);
    let mut words = match &first.why {
        Why::Zerons(title) => format!("Its {key} for “{}” is already Zeron's ({title}), so it's in the menu bar without one.", first.title),
        Why::Plugins(other) => format!("Its {key} for “{}” is already {}'s, so it's in the menu bar without one.", first.title, name_of(other)),
        Why::NoKey => format!("Its {key} for “{}” isn't a key a menu can show, so it's in the menu bar without one.", first.title),
        Why::NoModifier => format!("Its {key} for “{}” has no ⌘, ⌃ or ⌥, so it would take that key from everything you type in. It's in the menu bar without one.", first.title),
    };
    match lost.len() {
        1 => {}
        2 => words.push_str(" So is one more of its commands."),
        n => words.push_str(&format!(" So are {} more of its commands.", n - 1)),
    }
    words
}

/// A shortcut as the menu shows it: `cmd+shift+n` is ⇧⌘N.
pub(crate) fn shortcut_words(key: &str) -> String {
    let mut parts: Vec<&str> = key.split('+').map(str::trim).collect();
    let last = parts.pop().unwrap_or("");
    let mut out = String::new();
    for (names, mark) in [(&["ctrl", "control"][..], "⌃"), (&["alt", "opt", "option"][..], "⌥"), (&["shift"][..], "⇧"), (&["cmd", "command", "meta", "super"][..], "⌘")] {
        if parts.iter().any(|p| names.contains(&p.to_ascii_lowercase().as_str())) {
            out.push_str(mark);
        }
    }
    out.push_str(&last.to_uppercase());
    out
}

/// Zeron's design system lent to every plugin, those loaded later too: a
/// plugin requires `@ds/…` into its own VM (zgx's `Capability::Package`),
/// from the app running now, beside `entry` (the main window's from
/// `.live`), so its components match whichever template is in use. It
/// lends components only, no authority: what they call is gated by the
/// plugin's own grants. (The app's `.luaurc` alias `ds` names the same
/// folder, as zgx requires of a package and an alias of one name.)
pub fn lend_design_system(host: ScriptHost, entry: &Path) -> ScriptHost {
    let ui = entry.parent().map(Path::to_path_buf).unwrap_or_else(crate::ui_dir);
    let ds = ui.join("ds");
    let host = host.package("ds", ds).grant_all_plugins(Capability::Package("ds".into()));
    with_catalog(host, &ui)
}

/// Zeron's words (ui/locales) loaded by the host itself (zgx's
/// `ScriptHost::i18n`), before any script runs: zgx runs plugins before the
/// app, and keeps one catalog per host, so the design system lent to a
/// plugin shows them in the language chosen from the first load, as the app
/// does (ds/locale.luau finds the catalog loaded, and its own `ui.i18n` of
/// the same folder keeps it). No script of the host's own: a plugin loaded
/// makes every reload a full one. Without ui/locales (a bare test host),
/// none.
fn with_catalog(host: ScriptHost, ui: &Path) -> ScriptHost {
    let locales = ui.join("locales");
    if !locales.join("en.json").exists() {
        return host;
    }
    host.i18n(locales, "en")
}

/// Zeron's own plugins, shipped beside the app's scripts (`ui/`).
pub fn builtin_dir() -> PathBuf {
    crate::ui_dir().parent().map(|p| p.join("plugins-builtin")).unwrap_or_default()
}

/// Development only: the plugin folders `ZERON_ZGX_DEV_PLUGINS` names
/// (`:`-separated) loaded into a window opened by entry (a fixture), each
/// on and granted what its manifest asks, with no engine: for drawing a
/// plugin in a fixture (`zeron-zgx github_capture_fixture.luau --inspect`).
/// Its own actions are heard and dropped; the fixture plays the host.
/// `ZERON_ZGX_DEV_GITHUB` names a JSON file that plays GitHub for them
/// (`dev_github`).
pub fn dev_plugins(host: &ScriptHost) {
    let Some(dirs) = crate::dev::switch("ZERON_ZGX_DEV_PLUGINS") else { return };
    expose_surface(host);
    let plugin_network = network::Network::default();
    host.fetch_policy(plugin_network.clone());
    let mut hosts = Vec::new();
    let mut plugins = Vec::new();
    for dir in std::env::split_paths(&dirs) {
        let Ok(text) = std::fs::read_to_string(dir.join("zeron-plugin.json")) else {
            eprintln!("zeron-zgx: dev plugin {}: no zeron-plugin.json", dir.display());
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<Json>(&text) else { continue };
        let Some(id) = manifest["id"].as_str().map(str::to_string) else { continue };
        let item = Item {
            json: json!({
                "id": id, "name": manifest["name"], "status": "on", "entry": manifest["entry"],
                "snapshot": dir, "permissions": manifest["permissions"],
                "ui": manifest.get("ui").cloned().unwrap_or(json!({})),
                "network_hosts": manifest["network"]["hosts"],
            }),
        };
        hosts.push((id.clone(), item.network_hosts()));
        expose_own(host, &id);
        locales::publish(host, &id, &dir);
        // Its settings, at their defaults (a fixture sets them as it likes).
        let values = settings::values(&settings::declared(&id, &manifest, false), &Default::default());
        host.shared(&format!("plugin.{id}.settings"), json_value(&Json::Object(values)));
        for name in grants::OWN_ACTIONS {
            let key = format!("plugin.{id}.{name}");
            host.action(key.clone(), move |v: Value| eprintln!("zeron-zgx: dev plugin: {key} {}", v.to_json()));
        }
        // Its programs, run for real (those its manifest names), and its
        // database in a folder of the run's own, never a data dir's.
        let allowed: Vec<String> = manifest["commands"].as_array().into_iter().flatten().filter_map(|c| c.as_str().map(str::to_string)).collect();
        host.async_action(format!("plugin.{id}.run"), move |asked: Value| {
            let (asked, allowed) = (value_json(&asked), allowed.clone());
            async move { off_thread(move || commands::run(&asked, &allowed)).await.ok_or("The program couldn't be run")?.map(|v| json_value(&v)) }
        });
        let db_path = db::path_in(&std::env::temp_dir().join(format!("zgx-dev-plugins-{}", std::process::id())), &id);
        host.async_action(format!("plugin.{id}.db"), move |asked: Value| {
            let (asked, path) = (value_json(&asked), db_path.clone());
            async move {
                let path = path.ok_or("There's nowhere to keep it")?;
                off_thread(move || db::call(&path, &asked)).await.ok_or("The database couldn't be reached")?.map(|v| json_value(&v))
            }
        });
        dev_github(host, &id);
        dev_service(host, &id);
        eprintln!("zeron-zgx: dev plugin {id} from {}", dir.display());
        plugins.extend(item.plugin());
    }
    plugin_network.set(hosts);
    host.set_plugins(plugins);
}

/// Development only: GitHub played from the JSON file
/// `ZERON_ZGX_DEV_GITHUB` names, for a dev plugin `id` (the GitHub plugin
/// drawn in a fixture, or offscreen by src/audit_shots.rs): each call
/// answered by its `op` (`pr`, `diff`, `list`, `repository`, `file_url`),
/// writes as done, and `records` and `store` published as the bridge would.
fn dev_github(host: &ScriptHost, id: &str) {
    let Some(path) = crate::dev::switch("ZERON_ZGX_DEV_GITHUB") else { return };
    let answers: Json = match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string())) {
        Ok(json) => json,
        Err(error) => {
            eprintln!("zeron-zgx: ZERON_ZGX_DEV_GITHUB: {error}");
            return;
        }
    };
    for (key, name) in [("records", "records"), ("store", "store")] {
        if let Some(value) = answers.get(key) {
            host.shared(&format!("plugin.{id}.{name}"), json_value(value));
        }
    }
    // `run`: programs answered in place of running them, each by the words
    // its args and stdin hold (`{ "match": ["graphql", "pullRequests"],
    // "stdout": "…" }`, the first that matches), for what the plugin asks
    // through `gh` (its pull requests' watch); anything else runs for real.
    if let Some(played) = answers.get("run").and_then(Json::as_array).cloned() {
        let real: Vec<String> = vec!["gh".into()];
        host.async_action(format!("plugin.{id}.run"), move |asked: Value| {
            let asked = value_json(&asked);
            let all = format!("{} {}", asked["args"], asked["stdin"].as_str().unwrap_or_default());
            let hit = played.iter().find(|p| p["match"].as_array().into_iter().flatten().all(|w| w.as_str().is_some_and(|w| all.contains(w)))).cloned();
            eprintln!("zeron-zgx: dev plugin run {} {}", if hit.is_some() { "played" } else { "real" }, asked["args"]);
            let real = real.clone();
            async move {
                match hit {
                    Some(p) => Ok(json_value(&json!({ "status": 0, "stdout": p["stdout"].as_str().unwrap_or_default(), "stderr": "" }))),
                    None => off_thread(move || commands::run(&asked, &real)).await.ok_or("The program couldn't be run")?.map(|v| json_value(&v)),
                }
            }
        });
    }
    let answers = std::rc::Rc::new(answers);
    host.async_action(format!("plugin.{id}.github_call"), move |asked: Value| {
        let asked: Json = serde_json::from_str(&asked.to_json()).unwrap_or_default();
        let op = asked["op"].as_str().unwrap_or_default().to_string();
        let answer = answers.get(&op).cloned().unwrap_or_else(|| json!({ "ok": true }));
        async move { Ok(json_value(&answer)) }
    });
}

/// Development only: a device's plugin service played from the JSON file
/// `ZERON_ZGX_DEV_SERVICE` names, for a dev plugin `id` (the Git plugin
/// drawn in a fixture): each call (`zeron.service(device):call`) answered
/// by its method's entry (`changes` by its scope), `fail` (method →
/// "reason: words") refused as the engine refuses, anything else done.
fn dev_service(host: &ScriptHost, id: &str) {
    let Some(path) = crate::dev::switch("ZERON_ZGX_DEV_SERVICE") else { return };
    let answers: Json = match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string())) {
        Ok(json) => json,
        Err(error) => {
            eprintln!("zeron-zgx: ZERON_ZGX_DEV_SERVICE: {error}");
            return;
        }
    };
    if let Some(records) = answers.get("records") {
        host.shared(&format!("plugin.{id}.records"), Value::Nil).set(json_value(records));
    }
    let answers = std::rc::Rc::new(answers);
    host.async_action(format!("plugin.{id}.service_call"), move |asked: Value| {
        let asked: Json = serde_json::from_str(&asked.to_json()).unwrap_or_default();
        let method = asked["method"].as_str().unwrap_or_default().to_string();
        let answer = if let Some(why) = answers["fail"][&method].as_str() {
            Err(why.to_string())
        } else if method == "changes" {
            Ok(answers["changes"][asked["args"]["scope"].as_str().unwrap_or("working")].clone())
        } else {
            Ok(answers.get(&method).cloned().unwrap_or(json!(true)))
        };
        async move { answer.map(|v| json_value(&v)) }
    });
}

/// The main window's view (src/main.rs `serve`), for zgx's report of the
/// elements in it.
pub fn set_view(handle: zgx::compose::ViewHandle) {
    if let Some(b) = BRIDGE.with(|b| b.borrow().upgrade()) {
        *b.view.borrow_mut() = Some(handle);
        b.publish_page();
    }
}

/// "Use Zeron v3" (the host's own menu item, and Revert in its toast, which no
/// template can take away): safe mode (preview.rs), so Zeron's own runs
/// whatever `.active` says until you pick a template on the Templates page,
/// and the terminals templates started are closed. The window follows.
pub fn use_zeron_template() {
    preview::enter_safe();
    if let Some(root) = templates_root()
        && let Err(error) = live::set_active(&root, live::BUILTIN)
    {
        eprintln!("zeron-zgx: Zeron's template wasn't chosen: {error}");
    }
    if let Some(b) = BRIDGE.with(|b| b.borrow().upgrade()) {
        b.chose.set(true);
        b.look();
        b.rewatch();
    }
}

/// Who the sidebar says narrows it (`sidebar.filter_by`, the shell's
/// notice): the name, as Settings → Plugins shows it, of the one plugin
/// loading (`want`) that may set `sidebar.filter`; none (the shell's "A
/// plugin") for none or several. Never the `by` a plugin writes in the
/// filter, which could say it's Zeron or another plugin.
fn filter_by(want: &[Plugin], items: &[Item]) -> Value {
    let filter = Capability::Named("zeron.sidebar.filter".into());
    let mut allowed = want.iter().filter(|p| p.grants.contains(&filter));
    match (allowed.next(), allowed.next()) {
        (Some(one), None) => items.iter().find(|i| i.id() == one.name).map_or(Value::Nil, |i| Value::from(i.name())),
        _ => Value::Nil,
    }
}

/// Plugin `id`'s state in the window's host gone: its `ui.state` and
/// `ui.persist` values zgx holds, the writes it hasn't made yet (after which
/// none brings the folder back), and its saved folder. (Removing the folder
/// alone left zgx's values, so one put back this run had them again, and a
/// write still waiting wrote the folder back.)
pub(crate) fn forget_state(host: &ScriptHost, id: &str) {
    host.forget_state(&format!("plugin:{id}"));
    // (Where the host had no storage set, its folder as zgx names it.)
    if let Some(dir) = zgx_script::app_data_dir(crate::APP_ID) {
        let _ = std::fs::remove_dir_all(dir.join("script-state").join(format!("plugin%3A{id}")));
    }
}

/// What a plugin needed, as a person would say it: "a session".
/// What Settings → Plugins says of a plugin's node that couldn't show at
/// `at`: what it needed that the place doesn't give, or its error.
pub(crate) fn problem_words(plugin: &str, at: &str, need: Option<&str>, error: &str) -> String {
    let place = elements::place(at);
    match need {
        Some(need) if !need.is_empty() => format!("{plugin} needs {}, but it's in {place}", needed(need)),
        _ => format!("{plugin} couldn't show in {place}: {}", recovery::first_line(error)),
    }
}

fn needed(name: &str) -> String {
    match name {
        "session" => "a session".into(),
        "sessions" => "the sessions".into(),
        "message" => "a message".into(),
        "file" => "a file".into(),
        "panel" => "a panel".into(),
        "profile" => "your profile".into(),
        "project" => "a project".into(),
        other => format!("a {other}"),
    }
}

fn template_name(root: &Path, id: &str) -> String {
    live::installed(root).into_iter().find(|t| t.id == id).map(|t| t.name).unwrap_or_else(|| id.to_string())
}

/// The update the engine found for template `id` (`<templates>/.updates/
/// <id>.json`), while it's for the package installed (`hash`): its version,
/// and why it's refused when it is.
fn template_update(root: &Path, id: &str, hash: &str) -> Option<Json> {
    if hash.is_empty() {
        return None;
    }
    let found: Json = serde_json::from_slice(&std::fs::read(root.join(".updates").join(format!("{id}.json"))).ok()?).ok()?;
    if found["from"].as_str() != Some(hash) {
        return None;
    }
    let mut out = json!({ "version": found["version"].as_str().unwrap_or("") });
    if let Some(refused) = found["refused"].as_str() {
        out["refused"] = json!(refused);
    }
    Some(out)
}

/// A plugin's icon: the SVG its manifest names (`"icon": "icon.svg"`), a file
/// in its own folder (its checked copy once it has one), read whole; none if
/// it names none, or one outside its folder, too big, or not an SVG. Drawn
/// as a glyph in the theme's ink (screens/plugins/words `tile`).
pub(crate) fn icon_svg(item: &Json) -> Option<String> {
    const MAX: u64 = 32 * 1024;
    let dir = Path::new(item["snapshot"].as_str().or_else(|| item["dir"].as_str())?);
    let manifest = settings::manifest_in(dir)?;
    let rel = manifest["icon"].as_str()?;
    if rel.is_empty() || rel.starts_with('/') || rel.contains("..") || rel.contains('\\') || !rel.ends_with(".svg") {
        return None;
    }
    let path = dir.join(rel);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    text.trim_start().starts_with("<svg").then_some(text)
}

fn page_value(state: &str, dirs: Option<&(PathBuf, PathBuf)>, list: &[Item], templates: &[Value]) -> Value {
    let row = |i: &Item| {
        let mut json = i.json.clone();
        if let Some(svg) = icon_svg(&i.json) {
            json["icon_svg"] = Json::String(svg);
        }
        json_value(&json)
    };
    let mut fields = vec![
        ("state", Value::from(state)),
        ("list", vlist(list.iter().map(row).collect::<Vec<_>>())),
        ("templates", vlist(templates.to_vec())),
    ];
    if let Some((plugins, templates)) = dirs {
        fields.push((
            "dirs",
            Value::record([
                ("plugins", Value::from(plugins.to_string_lossy().as_ref())),
                ("templates", Value::from(templates.to_string_lossy().as_ref())),
            ]),
        ));
    }
    Value::record(fields)
}

impl Bridge {
    fn me(&self) -> Rc<Bridge> {
        self.me.borrow().upgrade().expect("the bridge is alive while its host is")
    }

    pub(crate) fn item(&self, id: &str) -> Option<Item> {
        self.items.borrow().iter().find(|i| i.id() == id).cloned()
    }

    /// Plugin `id` on `device` (none: this one), as its list says.
    pub(crate) fn item_on(&self, device: Option<&str>, id: &str) -> Option<Item> {
        match device {
            Some(device) => self.remote.item(device, id),
            None => self.item(id),
        }
    }

    /// Every module a plugin or the template stands in for now: where a
    /// screen that fails falls back to Zeron's own (screens/plugins).
    fn replaced(&self) -> Vec<Value> {
        let loaded = self.loaded.borrow();
        let mut out: Vec<Value> = self
            .items
            .borrow()
            .iter()
            .filter(|i| loaded.iter().any(|p| p.name == i.id()) && i.may("ui.replace"))
            .flat_map(|i| i.json["ui"]["replace"].as_array().cloned().unwrap_or_default())
            .filter_map(|m| m.as_str().map(Value::from))
            .collect();
        out.extend(
            self.overlay
                .borrow()
                .from_template
                .iter()
                .filter(|rel| rel.ends_with(".luau"))
                .map(|rel| Value::from(templates::module_of(rel).as_str())),
        );
        out
    }

    pub(crate) fn templates_dir(&self) -> Option<PathBuf> {
        self.templates_root.clone()
    }

    /// The engine's list, as it changes.
    fn heard(&self, frame: &Json) {
        let mut list: Vec<Item> = frame["plugins"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|json| Item { json: json.clone() })
            .collect();
        list.sort_by_key(|a| a.name().to_lowercase());
        if let (Some(p), Some(t)) = (frame["dirs"]["plugins"].as_str(), frame["dirs"]["templates"].as_str()) {
            *self.dirs.borrow_mut() = Some((PathBuf::from(p), PathBuf::from(t)));
        }
        // The engine's word on the account's signing keys (a key added on
        // another device, this device's revoked): said by the host, once.
        for notice in frame["key_notices"].as_array().into_iter().flatten() {
            if let (Some(id), Some(words)) = (notice["id"].as_str(), notice["words"].as_str())
                && self.told.borrow_mut().insert(format!("key-notice:{id}"))
            {
                host_bar::notice(host_bar::Notice::key(notice["kind"].as_str().unwrap_or(""), notice["device"].as_str(), words));
            }
        }
        // One removed, or put back from elsewhere under the same id, starts
        // afresh: nothing it left (its queues, what it set, its saved state)
        // reaches what's installed now.
        let gone = gone(&self.items.borrow(), &list);
        *self.items.borrow_mut() = list;
        for (id, old) in &gone {
            self.forget(id, old);
        }
        self.state.set("ready");
        // `ZERON_ZGX_REVIEW=<id>` (development): that plugin's sheet at
        // launch, its window serving devtools (run without --inspect).
        if let Some(id) = crate::dev::switch("ZERON_ZGX_REVIEW").and_then(|v| v.into_string().ok())
            && !self.reviewed.replace(true)
        {
            consent::review(&self.me(), &id, None);
        }
        if let Some(c) = &*self.consent.borrow() {
            c.follow(self);
        }
        self.apply();
        // One removed is unloaded now: what its VM held to the last (a
        // `ui.persist` it set as it went) goes too.
        for (id, _) in &gone {
            if !self.loaded.borrow().iter().any(|p| &p.name == id) {
                forget_state(&self.host, id);
            }
        }
    }

    /// Plugin `id` (as `old` was) is gone: what this side keeps of it goes
    /// too. Its calls and messages waiting, its records, its problems and
    /// failure, the sidebar's filter if it could set it, and its scripts'
    /// state (`ui.state`, `ui.persist`: what zgx holds of it this run, its
    /// writes waiting, and `<app data>/script-state/plugin:<id>/`), before
    /// the reload that unloads it or loads the one put back, so that one
    /// starts from its `initial`s. (Its storage is the engine's to remove.)
    fn forget(&self, id: &str, old: &Item) {
        if let Some(calls) = self.calls.borrow_mut().get_mut(id) {
            calls.list.clear();
        }
        if let Some(inbox) = self.inbox.borrow_mut().get_mut(id) {
            inbox.list.clear();
        }
        let shared: Vec<Shared> = [
            self.calls.borrow().get(id).and_then(|c| c.shared.clone()),
            self.inbox.borrow().get(id).and_then(|c| c.shared.clone()),
        ]
        .into_iter()
        .flatten()
        .collect();
        let records = self.records.borrow().get(id).cloned();
        self.problems.borrow_mut().remove(id);
        self.costs.borrow_mut().remove(id);
        self.failed_plugins.borrow_mut().forget(id);
        // (Set with no borrow held: a plugin's effects run at once.)
        for s in shared {
            s.set(vlist(Vec::new()));
        }
        if let Some(records) = records {
            records.set(Value::record([("profile", Value::record(Vec::<(&str, Value)>::new()))]));
        }
        let changed_sidebar = grants::capabilities(id, &old.permissions(), &old.change(), &[])
            .contains(&Capability::Named("zeron.sidebar.filter".into()));
        if changed_sidebar {
            self.host.shared("sidebar.filter", Value::Nil).set(Value::Nil);
        }
        forget_state(&self.host, id);
    }

    /// What zgx should load, from the list and your templates: set and
    /// reloaded when it changes.
    fn apply(&self) {
        let items = self.items.borrow().clone();
        let want: Vec<Plugin> = items
            .iter()
            .filter(|i| !self.failed_plugins.borrow_mut().holds(i.id(), i.hash()))
            .filter_map(Item::plugin)
            .collect();
        for plugin in &want {
            self.expose(&plugin.name);
        }
        // Where each may reach, before anything of it runs.
        self.network.set(items.iter().filter(|i| want.iter().any(|p| p.name == i.id())).map(|i| (i.id().to_string(), i.network_hosts())));
        // Each one's own words (`locales/`, read again: a new version brings
        // its own), for the SDK's `zeron.t`.
        for item in items.iter().filter(|i| want.iter().any(|p| p.name == i.id())) {
            if let Some(dir) = item.json["snapshot"].as_str() {
                locales::publish(&self.host, item.id(), Path::new(dir));
            }
        }
        // Menus of plugins no longer loaded go with them.
        let loading: HashSet<String> = want.iter().map(|p| p.name.clone()).collect();
        self.host.shared("sidebar.filter_by", Value::Nil).set(filter_by(&want, &items));
        let before = self.item_menus.borrow().len();
        self.item_menus.borrow_mut().retain(|id, _| loading.contains(id));
        if self.item_menus.borrow().len() != before {
            self.publish_menus();
        }
        // Its pull requests' watches too.
        self.gh.borrow_mut().retain(|id, _| loading.contains(id));
        // What it stored, as it is on disk now: removing a plugin takes its
        // storage (the engine's), so one installed again starts empty
        // rather than from what this run still held.
        for (id, (shared, held)) in self.stores.borrow_mut().iter_mut() {
            if !loading.contains(id) {
                let now = store::load(id);
                if *held != now {
                    *held = now;
                    shared.set(json_value(&Json::Object(held.clone())));
                }
            }
        }
        // Settings: a page each for those on that declare some.
        self.sync_settings(&items, &loading);
        // Pages too.
        let before = self.pages.borrow().len();
        self.pages.borrow_mut().retain(|id, _| loading.contains(id));
        if self.pages.borrow().len() != before {
            self.publish_pages();
        }
        self.publish_page();
        let (sources, list) = self.sources(false);
        self.publish_records(&sources, &list);
        if *self.loaded.borrow() == want {
            return;
        }
        *self.loaded.borrow_mut() = want.clone();
        self.host.set_plugins(want);
        self.reload();
    }

    fn reload(&self) {
        if let Err(error) = self.host.reload() {
            eprintln!("zeron-zgx: the window didn't reload with its plugins: {error}");
            self.blame(&error);
        }
    }

    /// `blame`, after this turn.
    /// A plugin watch answered `setup_incomplete`: Settings → Plugins says
    /// to finish setting up, and once the account is set up (an
    /// organization chosen) it's watched `again`, and the list follows.
    fn wait_for_setup(self: &Rc<Self>, again: crate::engine::rpc::Again) {
        if self.state.get() != "setup" {
            eprintln!("zeron-zgx: plugins: waiting for the account's setup to be finished");
            self.state.set("setup");
            self.publish_page();
        }
        let me = Rc::downgrade(self);
        self.tasks.spawn(async move {
            crate::auth::set_up().await;
            let Some(b) = me.upgrade() else { return };
            if b.state.get() == "setup" {
                b.state.set("loading");
                b.publish_page();
            }
            again();
        });
    }

    fn later_blame(&self, error: String) {
        let me = Rc::downgrade(&self.me());
        self.tasks.spawn(async move {
            if let Some(b) = me.upgrade() {
                b.blame(&error);
            }
        });
    }

    /// The template file or plugin `error` is on, if any, makes way. An
    /// error the window couldn't load past (`reload`'s) that isn't on one of
    /// the template's files is on its whole: Zeron's own comes back.
    fn blame(&self, error: &str) {
        self.blame_as(error, false);
    }

    fn blame_as(&self, error: &str, loading: bool) {
        // A preview asking for what it can't have yet isn't broken. (One that
        // can't load without it makes way like any, until it's kept.)
        if !loading && preview::refused(error) {
            return;
        }
        let loaded = self.loaded.borrow().clone();
        if let Some(id) = recovery::blame(error, &loaded) {
            self.plugin_failed(&id, error);
            return;
        }
        let Some(root) = self.templates_dir() else { return };
        let overlay = self.overlay.borrow().clone();
        if overlay.template == live::BUILTIN {
            return;
        }
        match live::blame(error, &root, &overlay) {
            Some(rel) if rel == "app.luau" => self.template_broken(error),
            Some(rel) => self.file_failed(&rel, error),
            None if loading => self.template_broken(error),
            None => {}
        }
    }

    /// Plugin `id` failed: withdrawn, everything it added with it, until a
    /// new version or an edit of it arrives.
    pub(crate) fn plugin_failed(&self, id: &str, error: &str) {
        let Some(item) = self.item(id) else { return };
        if !self.failed_plugins.borrow_mut().fail(id, item.hash(), &recovery::reason(error)) {
            return;
        }
        eprintln!("zeron-zgx: plugin {id} failed and is withdrawn: {}", recovery::first_line(error));
        let why = self.failed_plugins.borrow().why(id).unwrap_or_default().to_string();
        self.request(
            "plugin_failed",
            item.name(),
            Value::record([("id", Value::from(id)), ("error", Value::from(why.as_str()))]),
        );
        self.apply();
    }

    /// The active template's file `rel` failed: Zeron's own shows in its
    /// place until the file changes.
    fn file_failed(&self, rel: &str, error: &str) {
        let Some(root) = self.templates_dir() else { return };
        let id = self.overlay.borrow().template.clone();
        let version = live::file_stamp(&root.join(&id).join(rel));
        if !self.failed_files.borrow_mut().fail(&format!("{id}:{rel}"), &version, error) {
            return;
        }
        let n = {
            let mut fallbacks = self.fallbacks.borrow_mut();
            let n = fallbacks.entry(id.clone()).or_default();
            *n += 1;
            *n
        };
        if n > MOST_FALLBACKS {
            self.template_broken(error);
            return;
        }
        eprintln!("zeron-zgx: template {id}: {rel} failed, showing Zeron's own: {}", recovery::first_line(error));
        let what = if rel.ends_with(".luau") { templates::title(&templates::module_of(rel)) } else { rel.to_string() };
        self.request("template_failed", &what, Value::from(rel));
        self.look();
    }

    /// The active template can't run: the window goes back to Zeron's own
    /// whole, and the template shows as broken until it's edited.
    fn template_broken(&self, error: &str) {
        let Some(root) = self.templates_dir() else { return };
        let id = self.overlay.borrow().template.clone();
        if id == live::BUILTIN {
            return;
        }
        if !self.broken.borrow_mut().fail(&id, &live::template_stamp(&root.join(&id)), error) {
            return;
        }
        eprintln!("zeron-zgx: template {id} can't run, showing Zeron's own: {}", recovery::first_line(error));
        let name = template_name(&root, &id);
        self.request("template_broken", &name, Value::from(id.as_str()));
        self.look();
    }

    /// The window's interface, brought up to date: Zeron's own as this build
    /// has it, the active template on top (less what failed), and reloaded
    /// if anything in it changed.
    ///
    /// The file work runs off this thread, in two steps: a survey (`ui/`
    /// and the template as they stand, by a stat each; `.zeron` copied again
    /// only if `ui/` changed), then, if what `.live` is made from changed,
    /// the compose. What failed is decided here between them. One look at
    /// a time; one asked meanwhile runs after.
    pub(crate) fn look(&self) {
        if self.looking.replace(true) {
            self.look_again.set(true);
            return;
        }
        let Some(root) = self.templates_dir() else {
            self.looking.set(false);
            return;
        };
        let me = Rc::downgrade(&self.me());
        let last = self.laid.borrow().ui_stamp.clone();
        self.tasks.spawn(async move {
            let ui = crate::ui_dir();
            let survey = {
                let root = root.clone();
                off_thread(move || live::survey(&ui, &root, (!last.is_empty()).then_some(last.as_str()))).await
            };
            let Some(b) = me.upgrade() else { return };
            if let Some(survey) = survey {
                b.clone().looked(&root, survey).await;
            }
            b.looking.set(false);
            if b.look_again.replace(false) {
                b.look();
            }
        });
    }

    /// A look's second half: what failed decided, `.live` composed if what
    /// it's made from changed, and the window told.
    async fn looked(self: Rc<Self>, root: &Path, survey: live::Survey) {
        if let Err(error) = &survey.synced {
            eprintln!("zeron-zgx: Zeron's own interface wasn't copied: {error}");
            return;
        }
        let id = survey.id.clone();
        let whole_failed = id != live::BUILTIN && self.broken.borrow_mut().holds(&id, &survey.template_stamp);
        let failed: BTreeSet<String> = {
            let mut failures = self.failed_files.borrow_mut();
            survey.files.iter().filter(|(rel, at)| failures.holds(&format!("{id}:{rel}"), at)).map(|(rel, _)| rel.clone()).collect()
        };
        let want = live::Laid { ui_stamp: survey.ui_stamp, id: id.clone(), template_stamp: survey.template_stamp, whole_failed, failed };
        let compose = *self.laid.borrow() != want;
        // Only Zeron's own ui/ changed (a save while developing): the host
        // watches `.live`, so the files laid down reload it once, as any
        // save. A template switched, edited or set aside reloads here, its
        // failure to load blamed on the template (`blame_as`, loading).
        let template = {
            let l = self.laid.borrow();
            l.id != want.id || l.template_stamp != want.template_stamp || l.whole_failed != want.whole_failed || l.failed != want.failed
        };
        // Whether it runs with Zeron's powers (preview.rs): known before the
        // window reloads into it, so a preview's first line already runs
        // gated. Hashed again only when its files or what you kept change.
        let approvals = preview::approvals_path().map(|p| live::file_stamp(&p)).unwrap_or_default();
        let trust_key = (id.clone(), want.template_stamp.clone(), approvals);
        let cached = self.trust.borrow().as_ref().filter(|(k, _)| *k == trust_key).map(|(_, t)| *t);
        let trusted = match cached {
            Some(t) => t,
            None => {
                let (root, id) = (root.to_path_buf(), id.clone());
                let t = off_thread(move || preview::trusted(&root, &id)).await.unwrap_or(false);
                *self.trust.borrow_mut() = Some((trust_key, t));
                t
            }
        };
        let was = self.laid.borrow().id.clone();
        let name = template_name(root, &id);
        preview::set_template_running(id != live::BUILTIN);
        // What a preview saved is kept with it, or goes with it; what failed
        // only because it was one is tried again (`Bridge::blame_as`).
        let was_previewing = preview::current();
        let kept = trusted && was_previewing.as_ref().is_some_and(|p| p.id == id);
        if kept {
            self.host.keep_held_state();
        } else if was_previewing.as_ref().is_some_and(|p| trusted || p.id != id) {
            self.host.drop_held_state();
        }
        preview::set((!trusted).then(|| preview::Preview { id: id.clone(), name: name.clone() }));
        let retry = kept && {
            let files = self.failed_files.borrow_mut().forget_under(&format!("{id}:"));
            let whole = self.broken.borrow().why(&id).is_some();
            self.broken.borrow_mut().forget(&id);
            self.fallbacks.borrow_mut().remove(&id);
            files || whole
        };
        // A switch you didn't make here (an agent's use_template, a file
        // written by hand): said by the host, whatever the template shows.
        if was != id && !self.chose.replace(false) {
            let by_chat = std::fs::read(root.join(".active.by.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<Json>(&b).ok())
                .is_some_and(|v| v["chat"].as_str().is_some_and(|c| !c.is_empty()));
            host_bar::notice(host_bar::Notice::switched(&name, id == live::BUILTIN, by_chat, !trusted));
        }
        let laid = {
            let (root, want) = (root.to_path_buf(), want.clone());
            off_thread(move || compose.then(|| live::compose(&root, &want.id, &want.failed, want.whole_failed).map_err(|e| e.to_string())))
            .await
        };
        let mut changed = 0;
        match laid.flatten() {
            Some(Ok((n, overlay))) => {
                changed = n;
                *self.overlay.borrow_mut() = overlay;
                *self.laid.borrow_mut() = want;
            }
            Some(Err(error)) => eprintln!("zeron-zgx: the template wasn't laid over Zeron's own: {error}"),
            None => {}
        }
        self.publish_templates();
        self.publish_page();
        if (kept || (changed > 0 && template))
            && let Err(error) = self.host.reload()
        {
            eprintln!("zeron-zgx: the window didn't reload: {}", recovery::first_line(&error));
            self.blame_as(&error, true);
        }
        // What failed as a preview, laid down as it is now.
        if retry {
            self.look();
        }
    }

    /// The watched files looked at again (a switch changes which).
    fn rewatch(&self) {
        if let Some(rewatch) = &*self.rewatch.borrow() {
            rewatch.send(());
        }
    }

    /// The templates page's data: each template, the active one, and how
    /// each stands.
    fn publish_templates(&self) {
        let Some(root) = self.templates_dir() else { return };
        let active = self.overlay.borrow().template.clone();
        let mut list = Vec::new();
        let mut status = serde_json::Map::new();
        let installed = live::installed(&root);
        // Each template's own theme (its theme.luau, as data), for the Themes
        // page (src/themes.rs).
        crate::themes::set_templates(
            installed
                .iter()
                .filter(|t| t.id != live::BUILTIN)
                .map(|t| crate::themes::TemplateTheme { template: t.id.clone(), name: t.name.clone(), file: self.looks.theme_file(&root, &t.id) })
                .collect(),
        );
        for t in installed {
            let broken = self.broken.borrow().why(&t.id).map(String::from);
            let errors: BTreeMap<String, String> = live::files(&root.join(&t.id), |_| true)
                .keys()
                .filter_map(|rel| self.failed_files.borrow().why(&format!("{}:{rel}", t.id)).map(|why| (rel.clone(), why.to_string())))
                .collect();
            let (state, error) = if let Some(why) = &broken {
                ("broken", Some(why.clone()))
            } else if let Some((rel, why)) = errors.iter().next() {
                ("errors", Some(format!("{rel}: {why}")))
            } else if t.out_of_date {
                ("out_of_date", None)
            } else {
                ("ok", None)
            };
            if broken.is_some() || !errors.is_empty() {
                let mut entry = json!({ "files": errors });
                if let Some(why) = &broken {
                    entry["root"] = Json::String(why.clone());
                }
                status.insert(t.id.clone(), entry);
            }
            let mut fields = vec![
                ("id", Value::from(t.id.as_str())),
                ("name", Value::from(t.name.as_str())),
                ("description", Value::from(t.description.as_str())),
                ("author", Value::from(t.author.as_str())),
                ("version", Value::from(t.version.as_str())),
                ("zeron_version", Value::from(t.zeron_version.as_str())),
                ("builtin", Value::from(t.id == live::BUILTIN || t.official.is_some())),
                ("active", Value::from(t.id == active)),
                ("status", Value::from(state)),
            ];
            if let Some(error) = error {
                fields.push(("error", Value::from(error.as_str())));
            }
            // The colours its preview is drawn in (looks.rs): none for one
            // that changes none of Zeron's.
            let look = self.looks.look(&root, &t.id);
            if !look.is_null() {
                fields.push(("look", json_value(&look)));
            }
            // Someone's, from a package: who signed it, where it came from
            // (never a local path), and a newer version found there.
            if let Some(imported) = &t.imported {
                let mut imported = imported.clone();
                if let Some(source) = imported.get_mut("source").and_then(Json::as_object_mut) {
                    source.retain(|k, _| matches!(k.as_str(), "kind" | "url" | "ref"));
                }
                if let Some(found) = template_update(&root, &t.id, imported["hash"].as_str().unwrap_or("")) {
                    fields.push(("update", json_value(&found)));
                }
                fields.push(("imported", json_value(&imported)));
            }
            list.push(Value::record(fields));
        }
        // Packages waiting for you to allow them (an agent's import_template,
        // or one brought in here). `here`: its window opened as it came, so
        // the app gives it no notice as well (screens/plugins/waiting).
        let mut pending = live::pending(&root);
        for p in &mut pending {
            let name = p["pending"].as_str().unwrap_or("").to_string();
            p["here"] = json!(self.told.borrow().contains(&format!("template:{name}")));
        }
        templates::write_if_changed(&root.join(live::STATUS), &Json::Object(status));
        self.templates_page.set(Value::record([
            ("state", Value::from("ready")),
            ("active", Value::from(active.as_str())),
            ("dir", Value::from(root.to_string_lossy().as_ref())),
            ("list", vlist(list)),
            ("pending", vlist(pending.iter().map(json_value).collect())),
        ]));
    }

    fn publish_page(&self) {
        // A plugin that failed here shows so, with why, whatever the engine
        // says of it; one placed where the template in use has nothing says
        // where it won't show, and one whose place zgx refused says why.
        // What the window running now names (its template's, or Zeron's),
        // as zgx reports it, and what each contribution does now.
        let (ids, classes, tree) = self.elements_report().map(|r| elements::from_report(&r)).unwrap_or_default();
        let contributions = self.host.contributions();
        let items: Vec<Item> = self
            .items
            .borrow()
            .iter()
            .map(|i| {
                let mut i = i.clone();
                if let Some(why) = self.failed_plugins.borrow().why(i.id()) {
                    i.json["failed"] = Json::String(why.to_string());
                }
                // A shortcut it lost to Zeron's (or another plugin's).
                if let Some(lost) = SHORTCUTS_TAKEN.with(|t| t.borrow().get(i.id()).cloned())
                    && !lost.is_empty()
                {
                    let name_of = |id: &str| self.item(id).map_or_else(|| id.to_string(), |p| p.name().to_string());
                    i.json["problem"] = Json::String(shortcut_notice(&lost, name_of));
                }
                if let Some((hash, words)) = self.problems.borrow().get(i.id())
                    && hash == i.hash()
                {
                    i.json["problem"] = Json::String(words.clone());
                }
                if i.status() == "on" {
                    let mine: Vec<&zgx_script::ContributionReport> = contributions.iter().filter(|c| c.plugin == i.id()).collect();
                    // Matching nothing now, and named nowhere in the window
                    // (a page not open now still names its parts).
                    let nowhere: std::collections::BTreeSet<String> = mine
                        .iter()
                        .filter(|c| c.matches == 0 && c.error.is_none() && !ids.is_empty())
                        .map(|c| c.target.clone())
                        .collect();
                    let mut missing: Vec<Json> = Vec::new();
                    for target in elements::not_shown(&nowhere, &ids, &classes) {
                        let place = Json::String(elements::place(&target).to_string());
                        if !missing.contains(&place) {
                            missing.push(place);
                        }
                    }
                    if !missing.is_empty() {
                        i.json["hidden"] = Json::Array(missing);
                    }
                    if i.json["problem"].is_null()
                        && let Some(c) = mine.iter().find(|c| c.error.is_some())
                    {
                        i.json["problem"] = Json::String(problem_words(i.name(), &c.target, None, c.error.as_deref().unwrap_or("")));
                    }
                    // Past its budget: said, if nothing worse is.
                    if i.json["problem"].is_null()
                        && let Some(slow) = self.costs.borrow().get(i.id()).and_then(surfaces::cost_words)
                    {
                        i.json["problem"] = Json::String(slow);
                    }
                }
                i
            })
            .collect();
        let mut page = page_value(self.state.get(), self.dirs.borrow().as_ref(), &items, &[]);
        if let Value::Map(fields) = &page {
            let mut fields: Vec<(std::rc::Rc<str>, Value)> = fields.to_vec();
            fields.push(("replaced".into(), vlist(self.replaced())));
            page = Value::record(fields);
        }
        self.page.set(page);
        if !ids.is_empty() {
            write_elements(&elements::describe(&ids, &classes, tree));
        }
    }

    /// zgx's report of the window's elements, once its view is known.
    fn elements_report(&self) -> Option<zgx_script::ElementsReport> {
        let ui = self.ui.upgrade()?;
        let view = self.view.borrow();
        Some(self.host.elements(&ui, view.as_ref()?))
    }

    /// What records are made from, as the window has it: followed (`track`,
    /// from the records' effect) or only read. The open chat's messages are
    /// followed only while a plugin may read them.
    fn sources(&self, track: bool) -> (records::Sources, Value) {
        let read = |key: &str| {
            let signal = self.host.shared(key, Value::Nil).signal();
            let value = if track { signal.get() } else { signal.with_untracked(Value::clone) };
            value_json(&value)
        };
        let messages_wanted = self.items.borrow().iter().any(|i| i.status() == "on" && records::reads_messages(&i.permissions()));
        // (No plugin reading them: nothing is made of a whole transcript.)
        let messages = if messages_wanted { read("chat.messages") } else { Json::Null };
        // The session list itself: records are made from it directly (no text of a
        // whole account, on the main thread, each time the list moves).
        let signal = self.host.shared("sessions", Value::Nil).signal();
        let all = if track { signal.get() } else { signal.with_untracked(Value::clone) };
        let sources = records::Sources {
            sessions: Json::Null,
            messages,
            profile: read("plugins.profile"),
            hide_emails: read("plugins.hide_emails").as_bool().unwrap_or(false),
        };
        (sources, all)
    }

    /// Each plugin's records, from `sources`, holding what it may read.
    fn publish_records(&self, sources: &records::Sources, list: &Value) {
        let items = self.items.borrow().clone();
        let shared: Vec<(Shared, Value)> = items
            .iter()
            .filter(|i| i.status() == "on")
            .filter_map(|i| {
                let shared = self.records.borrow().get(i.id()).cloned()?;
                Some((shared, records::published(&i.permissions(), &i.change(), sources, list)))
            })
            .collect();
        // (Set with no borrow held: a plugin's effects run at once.)
        for (shared, value) in shared {
            shared.set(value);
        }
    }

    /// A plugin's node couldn't be built where it was placed: said once, in
    /// words, on its row in Settings → Plugins.
    fn problem(&self, plugin: &str, problem: &Value) {
        let Some(item) = self.item(plugin) else { return };
        let words = problem_words(
            item.name(),
            &problem.field("at").map(text).unwrap_or_default(),
            problem.field("need").map(text).as_deref(),
            &problem.field("error").map(text).unwrap_or_default(),
        );
        let mut problems = self.problems.borrow_mut();
        if problems.get(plugin).is_some_and(|(hash, said)| hash == item.hash() && *said == words) {
            return;
        }
        eprintln!("zeron-zgx: plugin {plugin}: {words}");
        problems.insert(plugin.to_string(), (item.hash().to_string(), words));
        drop(problems);
        self.publish_page();
    }

    /// Something for the app to do (screens/plugins runs each once).
    fn request(&self, kind: &str, title: &str, value: Value) {
        let seq = self.request_seq.get() + 1;
        self.request_seq.set(seq);
        let mut list = self.request_list.borrow_mut();
        list.push(Value::record([
            ("seq", Value::from(seq as f64)),
            ("kind", Value::from(kind)),
            ("title", Value::from(title)),
            ("value", value),
        ]));
        let len = list.len();
        if len > REQUESTS {
            list.drain(..len - REQUESTS);
        }
        let value = vlist(list.clone());
        drop(list);
        self.requests.set(value);
    }

    fn publish_menus(&self) {
        let all: Vec<Value> = self.item_menus.borrow().values().flat_map(|items| items.iter().cloned()).collect();
        // Each plugin's own menus' items, to it alone (plugin_menus).
        for plugin in self.exposed.borrow().iter() {
            self.host.shared(&format!("plugin.{plugin}.menu_items"), vlist(Vec::new())).set(vlist(plugin_menus::offered_to(plugin, &all)));
        }
        self.menus.set(vlist(all));
    }

    // -- calls --------------------------------------------------------------

    // (Setting a plugin's shared state runs its effects at once, and a
    // plugin answers from them: no borrow is held across a `set`.)

    fn push_call(&self, plugin: &str, id: String, kind: &str, fields: Vec<(&str, Value)>) {
        let publish = {
            let mut calls = self.calls.borrow_mut();
            let entry = calls.entry(plugin.into()).or_default();
            let mut record = vec![("id", Value::from(id.as_str())), ("kind", Value::from(kind))];
            record.extend(fields);
            entry.list.push((id, Value::record(record)));
            entry.shared.clone().map(|s| (s, vlist(entry.list.iter().map(|(_, v)| v.clone()).collect())))
        };
        if let Some((shared, value)) = publish {
            shared.set(value);
        }
    }

    fn take_call(&self, plugin: &str, id: &str) -> Option<Value> {
        let (call, publish) = {
            let mut calls = self.calls.borrow_mut();
            let entry = calls.get_mut(plugin)?;
            let at = entry.list.iter().position(|(c, _)| c == id)?;
            let (_, call) = entry.list.remove(at);
            let publish = entry.shared.clone().map(|s| (s, vlist(entry.list.iter().map(|(_, v)| v.clone()).collect())));
            (call, publish)
        };
        if let Some((shared, value)) = publish {
            shared.set(value);
        }
        Some(call)
    }

    /// An agent's call of a plugin's tool, from the engine.
    fn tool_call(&self, call: &Json) {
        if std::env::var_os("ZERON_ZGX_PLUGIN_LOG").is_some() {
            eprintln!("zeron-zgx: plugins: a tool call: {call}");
        }
        let id = call["callId"].as_str().unwrap_or("").to_string();
        let plugin = call["plugin"].as_str().unwrap_or("").to_string();
        if id.is_empty() {
            return;
        }
        if let Some(why) = self.failed_plugins.borrow().why(&plugin) {
            let name = self.item(&plugin).map(|i| i.name().to_string()).unwrap_or(plugin.clone());
            answer_tool(id, false, Json::Null, format!("{name} stopped working: {why}"));
            return;
        }
        // An agent asking where the plugin's nodes are (`plugin_status`).
        if let Some(tool @ ("@status" | "@snapshot")) = call["tool"].as_str() {
            self.status_call(&plugin, id, tool == "@snapshot");
            return;
        }
        let loaded = self.loaded.borrow().iter().any(|p| p.name == plugin && plugin != "templates");
        if !loaded {
            let name = self.item(&plugin).map(|i| i.name().to_string()).unwrap_or(plugin.clone());
            answer_tool(id, false, Json::Null, format!("{name} isn't on in Zeron right now"));
            return;
        }
        let mut fields = vec![
            ("tool", Value::from(call["tool"].as_str().unwrap_or(""))),
            ("args", json_value(&call["args"])),
        ];
        if let Some(chat) = call["chatId"].as_str() {
            fields.push(("chat", Value::from(chat)));
        }
        self.push_call(&plugin, id, "tool", fields);
    }

    /// The plugin answered a call.
    fn result(&self, plugin: &str, answer: &Value) {
        if std::env::var_os("ZERON_ZGX_PLUGIN_LOG").is_some() {
            eprintln!("zeron-zgx: plugins: {plugin} answered {}", answer.to_json());
        }
        let id = answer.field("id").map(text).unwrap_or_default();
        let Some(call) = self.take_call(plugin, &id) else { return };
        if call.field("kind").map(text).as_deref() == Some("where") {
            self.status_answer(plugin, id, &call, answer);
            return;
        }
        if call.field("kind").map(text).as_deref() != Some("tool") {
            return;
        }
        let ok = answer.field("ok").is_some_and(Value::truthy);
        let value = answer.field("value").map(value_json).unwrap_or(Json::Null);
        let error = answer.field("error").map(text).unwrap_or_default();
        answer_tool(id, ok, value, error);
    }

    /// A message from the same plugin on another of your devices.
    fn message(&self, message: &Json) {
        let plugin = message["plugin"].as_str().unwrap_or("").to_string();
        let mut inbox = self.inbox.borrow_mut();
        let entry = inbox.entry(plugin).or_default();
        entry.seq += 1;
        let mut fields = vec![
            ("seq", Value::from(entry.seq as f64)),
            ("from", json_value(&message["from"])),
            ("data", json_value(&message["data"])),
            ("at", Value::from(message["at"].as_f64().unwrap_or(0.))),
        ];
        // A service's event (`zeron.emit`), for `zeron.service(device):on`;
        // `here`: this device's own service.
        if let Some(event) = message["event"].as_str() {
            fields.push(("event", Value::from(event)));
            fields.push(("here", Value::from(message["here"].as_bool().unwrap_or(false))));
        }
        entry.list.push(Value::record(fields));
        let len = entry.list.len();
        if len > INBOX {
            entry.list.drain(..len - INBOX);
        }
        let publish = entry.shared.clone().map(|s| (s, vlist(entry.list.clone())));
        drop(inbox);
        if let Some((shared, value)) = publish {
            shared.set(value);
        }
    }

    fn ack(&self, plugin: &str, seq: f64) {
        let publish = {
            let mut inbox = self.inbox.borrow_mut();
            let Some(entry) = inbox.get_mut(plugin) else { return };
            entry.list.retain(|m| m.field("seq").and_then(as_f64).is_some_and(|s| s > seq));
            entry.shared.clone().map(|s| (s, vlist(entry.list.clone())))
        };
        if let Some((shared, value)) = publish {
            shared.set(value);
        }
    }

    // -- the actions --------------------------------------------------------

    /// Plugin `id`'s own shared state and actions, once.
    fn expose(&self, id: &str) {
        if !self.exposed.borrow_mut().insert(id.to_string()) {
            return;
        }
        let host = &self.host;
        let calls_key = format!("plugin.{id}.calls");
        let inbox_key = format!("plugin.{id}.inbox");
        expose_own(host, id);
        {
            let shared = host.shared(&format!("plugin.{id}.records"), Value::Nil);
            shared.set(Value::record([("profile", Value::record(Vec::<(&str, Value)>::new()))]));
            self.records.borrow_mut().insert(id.to_string(), shared);
        }
        {
            let mut calls = self.calls.borrow_mut();
            let entry = calls.entry(id.into()).or_default();
            let shared = host.shared(&calls_key, vlist(Vec::new()));
            shared.set(vlist(entry.list.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>()));
            entry.shared = Some(shared);
        }
        {
            let mut inbox = self.inbox.borrow_mut();
            let entry = inbox.entry(id.into()).or_default();
            let shared = host.shared(&inbox_key, vlist(Vec::new()));
            shared.set(vlist(entry.list.clone()));
            entry.shared = Some(shared);
        }
        let plugin = id.to_string();
        let me = Rc::downgrade(&self.me());
        host.action(format!("plugin.{id}.result"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |answer| {
                if let Some(b) = me.upgrade() {
                    b.result(&plugin, &answer);
                }
            }
        });
        host.action(format!("plugin.{id}.ack"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |seq| {
                if let Some(b) = me.upgrade() {
                    b.ack(&plugin, as_f64(&seq).unwrap_or(0.));
                }
            }
        });
        host.async_action(format!("plugin.{id}.send"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |arg| {
                let (me, plugin) = (me.clone(), plugin.clone());
                async move {
                    let allowed = me.upgrade().and_then(|b| b.item(&plugin)).is_some_and(|i| i.may("devices"));
                    if !allowed {
                        return Err("This plugin wasn't allowed to send to your devices".to_string());
                    }
                    let to = arg.field("to").map(text).filter(|t| !t.is_empty()).unwrap_or_else(|| "*".into());
                    let data = arg.field("data").map(value_json).unwrap_or(Json::Null);
                    let reply = engine::get()
                        .call("PluginSend", json!({ "plugin": plugin, "to": to, "data": data }))
                        .await
                        .map_err(|e| engine::outcome(&e).to_string())?;
                    Ok(json_value(&reply["results"]))
                }
            }
        });
        // Its service (`zeron.service(device?):call`): on this device, or on
        // another of yours. A refusal fails the promise as "<reason>: <words>".
        host.async_action(format!("plugin.{id}.service_call"), {
            let plugin = plugin.clone();
            move |asked| {
                let plugin = plugin.clone();
                async move {
                    let asked = value_json(&asked);
                    let mut params = json!({ "plugin": plugin, "method": asked["method"], "args": asked["args"] });
                    if let Some(device) = asked["device"].as_str().filter(|d| !d.is_empty()) {
                        params["device"] = json!(device);
                    }
                    if let Some(timeout) = asked["timeout"].as_f64() {
                        params["timeout"] = json!(timeout);
                    }
                    let reply = call("CallPluginService", params).await.map_err(|e| {
                        if e.contains("unknown method") {
                            "old_engine: This device's Zeron is from before plugin services".to_string()
                        } else {
                            e
                        }
                    })?;
                    if reply["ok"].as_bool() == Some(true) {
                        Ok(json_value(&reply["value"]))
                    } else {
                        Err(format!(
                            "{}: {}",
                            reply["reason"].as_str().unwrap_or("failed"),
                            reply["message"].as_str().unwrap_or("The service didn't answer")
                        ))
                    }
                }
            }
        });
        // Its agents' usage, accounts and models (`zeron.usage`,
        // `zeron.switch_account`, `zeron.set_model`, `zeron.on`, …): the
        // engine checks what this device, or the device named, allowed it
        // (plugins/agents there). A refusal fails the promise as "<reason>:
        // <words>". A default for new chats the engine allowed is this
        // window's to keep: the composer takes it (`plugins.new_chat_model`).
        host.async_action(format!("plugin.{id}.agents"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |asked| {
                let (me, plugin) = (me.clone(), plugin.clone());
                async move {
                    let asked = value_json(&asked);
                    let op = asked["op"].as_str().unwrap_or_default().to_owned();
                    let mut params = json!({ "plugin": plugin, "op": op, "args": asked["args"] });
                    let device = asked["device"].as_str().filter(|d| !d.is_empty()).map(str::to_owned);
                    if let Some(device) = &device {
                        params["device"] = json!(device);
                    }
                    if op == "listen" {
                        // (An empty list may come as an empty table: none.)
                        params["listeners"] = if asked["listeners"].is_array() { asked["listeners"].clone() } else { json!([]) };
                    }
                    let reply = call("PluginAgents", params).await.map_err(|e| {
                        if e.contains("unknown method") {
                            "old_engine: This device's Zeron is from before plugins could read usage".to_string()
                        } else {
                            e
                        }
                    })?;
                    if reply["ok"].as_bool() != Some(true) {
                        return Err(format!(
                            "{}: {}",
                            reply["reason"].as_str().unwrap_or("failed"),
                            reply["message"].as_str().unwrap_or("Zeron didn't answer")
                        ));
                    }
                    let value = &reply["value"];
                    if op == "set_default_model"
                        && device.is_none()
                        && let (Some(b), Some(harness), Some(model)) =
                            (me.upgrade(), value["provider"].as_str(), value["model"].as_str())
                    {
                        let at = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.as_millis() as i64);
                        b.host.shared("plugins.new_chat_model", Value::Nil).set(json_value(&json!({
                            "model": format!("{harness}/{model}"),
                            "effort": value["effort"],
                            "seq": at,
                            "by": plugin,
                        })));
                    }
                    Ok(json_value(value))
                }
            }
        });
        host.action(format!("plugin.{id}.menus"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |list| {
                if let Some(b) = me.upgrade() {
                    b.set_menus(&plugin, &list);
                }
            }
        });
        // An item another plugin added to one of this plugin's menus, chosen
        // there: run in the plugin that added it (plugin_menus).
        host.action(format!("plugin.{id}.menu_run"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |asked: Value| {
                let Some(b) = me.upgrade() else { return };
                let all: Vec<Value> = b.item_menus.borrow().values().flat_map(|items| items.iter().cloned()).collect();
                let Some((target, item)) = plugin_menus::route(&plugin, &asked, &all) else { return };
                if !b.loaded.borrow().iter().any(|p| p.name == target) {
                    return;
                }
                let context = asked.field("context").cloned().unwrap_or(Value::Nil);
                b.push_call(&target, format!("menu-{}", next_id()), "menu", vec![("item", Value::from(item.as_str())), ("args", context)]);
            }
        });
        host.action(format!("plugin.{id}.problem"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |problem| {
                if let Some(b) = me.upgrade() {
                    // (Told from a build: taken after it.)
                    let (me, plugin) = (Rc::downgrade(&b), plugin.clone());
                    b.tasks.spawn(async move {
                        if let Some(b) = me.upgrade() {
                            b.problem(&plugin, &problem);
                        }
                    });
                }
            }
        });
        host.action(format!("plugin.{id}.toast"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |message| {
                if let Some(b) = me.upgrade() {
                    b.toast(&plugin, &text(&message));
                }
            }
        });
        {
            let shared = host.shared(&format!("plugin.{id}.github"), Value::record(Vec::<(&str, Value)>::new()));
            self.gh.borrow_mut().entry(id.to_string()).or_default().shared = Some(shared);
        }
        {
            let held = store::load(id);
            let shared = host.shared(&format!("plugin.{id}.store"), json_value(&Json::Object(held.clone())));
            shared.set(json_value(&Json::Object(held.clone())));
            self.stores.borrow_mut().insert(id.to_string(), (shared, held));
        }
        {
            let manifest = self.item(id).and_then(|i| i.json["snapshot"].as_str().map(PathBuf::from)).and_then(|s| settings::manifest_in(&s));
            let values = self.settings.borrow().values_from(id, manifest.as_ref());
            let shared = host.shared(&format!("plugin.{id}.settings"), json_value(&Json::Object(values.clone())));
            shared.set(json_value(&Json::Object(values)));
            self.settings_own.borrow_mut().insert(id.to_string(), shared);
        }
        host.action(format!("plugin.{id}.setting_status"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |asked| {
                if let Some(b) = me.upgrade() {
                    let asked = value_json(&asked);
                    let id = asked["id"].as_str().unwrap_or_default().to_string();
                    let status = if asked["status"].is_null() { Json::Null } else { asked["status"].clone() };
                    let done = b.settings.borrow_mut().set_status(&plugin, &id, &status);
                    match done {
                        Ok(true) => b.publish_settings(true, &[]),
                        Ok(false) => {}
                        Err(why) => eprintln!("zeron-zgx: plugin {plugin}: a setting's status wasn't shown: {why}"),
                    }
                }
            }
        });
        host.action(format!("plugin.{id}.store_set"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |asked| {
                if let Some(b) = me.upgrade() {
                    b.store_set(&plugin, &value_json(&asked));
                }
            }
        });
        // Its programs ("commands"): those its manifest names, run as you.
        host.async_action(format!("plugin.{id}.run"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |asked| {
                let (me, plugin) = (me.clone(), plugin.clone());
                async move {
                    let item = me.upgrade().and_then(|b| b.item(&plugin)).ok_or("This plugin isn't on")?;
                    if !item.may("commands") {
                        return Err("This plugin wasn't allowed to run programs".to_string());
                    }
                    let allowed: Vec<String> = item.json["commands"].as_array().into_iter().flatten().filter_map(|c| c.as_str().map(str::to_string)).collect();
                    let asked = value_json(&asked);
                    let ran = off_thread(move || commands::run(&asked, &allowed)).await.ok_or("The program couldn't be run")??;
                    Ok(json_value(&ran))
                }
            }
        });
        // Its database (`zeron.db`): its own, no grant.
        host.async_action(format!("plugin.{id}.db"), {
            let plugin = plugin.clone();
            move |asked| {
                let plugin = plugin.clone();
                async move {
                    let path = db::path(&plugin).ok_or("There's nowhere to keep it")?;
                    let asked = value_json(&asked);
                    let answer = off_thread(move || db::call(&path, &asked)).await.ok_or("The database couldn't be reached")??;
                    Ok(json_value(&answer))
                }
            }
        });
        host.async_action(format!("plugin.{id}.github_call"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |asked| {
                let (me, plugin) = (me.clone(), plugin.clone());
                async move {
                    let bridge = me.upgrade();
                    let item = bridge.as_ref().and_then(|b| b.item(&plugin));
                    let asked = value_json(&asked);
                    let (method, mut params) = github::call(&asked, |p| item.as_ref().is_some_and(|i| i.may(p)))?;
                    // Asked for a session's pull request: on the device its
                    // checkout lives on, as Zeron's own viewer asks.
                    let sessions = bridge
                        .as_ref()
                        .map(|b| value_json(&b.host.shared("sessions", Value::Nil).signal().with_untracked(Value::clone)))
                        .unwrap_or_default();
                    let device_of = |list: &str, id: &str| {
                        sessions[list].as_array().into_iter().flatten().find(|c| c["id"] == id).and_then(|c| c["device"].as_str()).filter(|d| !d.is_empty()).map(str::to_string)
                    };
                    if let (Some(session), Some(object)) = (asked["session"].as_str(), params.as_object_mut())
                        && let Some(device) = device_of("chats", session)
                    {
                        object.insert("targetDeviceId".into(), json!(device));
                    }
                    // A project's repository and pull requests: on the device
                    // its folder is on; the folder itself stays Zeron's.
                    if let (Some(project), Some(object)) = (asked["project"].as_str(), params.as_object_mut()) {
                        if let Some(device) = device_of("projects", project) {
                            object.insert("targetDeviceId".into(), json!(device));
                        }
                        if method == "GetChangeRequestRepository" {
                            let path = sessions["projects"].as_array().into_iter().flatten().find(|p| p["id"] == project).and_then(|p| p["path"].as_str()).map(str::to_string);
                            object.insert("cwd".into(), json!(path.ok_or("There's no such project")?));
                        }
                    }
                    if method == "GetChangeRequestRepository" && params.get("cwd").is_none() {
                        return Err("Which project's repository: its id".to_string());
                    }
                    // (Answered without the engine: a file's link.)
                    if method.is_empty() {
                        return Ok(json_value(&params));
                    }
                    let reply = call(method, params).await?;
                    Ok(json_value(&reply))
                }
            }
        });
        host.action(format!("plugin.{id}.github_watch"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |list| {
                if let Some(b) = me.upgrade() {
                    b.github_watch(&plugin, &value_json(&list));
                }
            }
        });
        host.action(format!("plugin.{id}.cost"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |cost| {
                if let Some(b) = me.upgrade() {
                    let cost = value_json(&cost);
                    let was = b.costs.borrow().get(&plugin).and_then(surfaces::cost_words);
                    let now = surfaces::cost_words(&cost);
                    b.costs.borrow_mut().insert(plugin.clone(), cost);
                    // (The row's words, again, only as they change; after
                    // this turn: told from a plugin's build.)
                    if was != now {
                        let me = Rc::downgrade(&b);
                        b.tasks.spawn(async move {
                            if let Some(b) = me.upgrade() {
                                b.publish_page();
                            }
                        });
                    }
                }
            }
        });
        host.async_action(format!("plugin.{id}.notify"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |asked| {
                let (me, plugin) = (me.clone(), plugin.clone());
                async move {
                    let item = me.upgrade().and_then(|b| b.item(&plugin)).ok_or("This plugin isn't on")?;
                    if !item.may("notifications") {
                        return Err("This plugin wasn't allowed to send you notifications".to_string());
                    }
                    let asked = value_json(&asked);
                    let title = asked["title"].as_str().unwrap_or_default().trim();
                    if title.is_empty() {
                        return Err("A notification needs a title".to_string());
                    }
                    // Said as the plugin's: "GitHub: …".
                    let mut params = json!({ "source": plugin, "title": format!("{}: {title}", item.name()), "body": asked["body"].as_str().unwrap_or_default() });
                    if let Some(session) = asked["session"].as_str().filter(|s| !s.is_empty()) {
                        params["session"] = json!(session);
                    }
                    let reply = call("PostNotification", params).await?;
                    Ok(json_value(&reply["outcome"]))
                }
            }
        });
        // A review comment on a file or a diff, onto a session's draft (as
        // the files pane's: screens/plugins hands it to the composer), for
        // a plugin allowed "compose". What it left that's still carried
        // comes back to it as `plugin.<id>.composed`.
        // Only on the chat shown, its code held to what's sent (`comment`).
        host.action(format!("plugin.{id}.compose_comment"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |asked| {
                if let Some(b) = me.upgrade()
                    && b.item(&plugin).is_some_and(|i| i.may("compose"))
                {
                    let shown = text(&b.host.shared("zeron.page_session", Value::Nil).signal().with_untracked(Value::clone));
                    match comment(&asked, &shown) {
                        Some(asked) => b.request("compose_comment", &plugin, asked),
                        None => eprintln!("zeron-zgx: plugin {plugin}: a comment for a chat not shown, refused"),
                    }
                }
            }
        });
        host.action(format!("plugin.{id}.open_url"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |url| {
                if let Some(b) = me.upgrade() {
                    b.open_url(&plugin, &text(&url));
                }
            }
        });
        host.action(format!("plugin.{id}.open_tab"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |asked| {
                if let Some(b) = me.upgrade() {
                    b.open_tab(&plugin, &value_json(&asked));
                }
            }
        });
        host.action(format!("plugin.{id}.add_page"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |asked| {
                if let Some(b) = me.upgrade() {
                    b.add_page(&plugin, &value_json(&asked));
                }
            }
        });
    }

    /// A web link a plugin opens ("links.open"): where Zeron opens links
    /// (Settings → Links' "Open links in": the panel's browser tab beside the
    /// session, or the default browser), `http:` and `https:` alone.
    fn open_url(&self, plugin: &str, url: &str) {
        let Some(item) = self.item(plugin) else { return };
        if !item.may("links.open") {
            eprintln!("zeron-zgx: plugin {plugin}: open_url without \"links.open\"");
            return;
        }
        if !surfaces::web_link(url) {
            eprintln!("zeron-zgx: plugin {plugin}: open_url takes http and https links");
            return;
        }
        let name = item.name().to_string();
        self.request("link", &name, Value::from(url));
    }

    /// A tab of the plugin's own in a chat's panel ("panel.tabs"): the app
    /// opens it (screens/plugins), and the panel draws its placeholder for
    /// the plugin to fill.
    fn open_tab(&self, plugin: &str, asked: &Json) {
        let Some(item) = self.item(plugin) else { return };
        if !item.may("panel.tabs") {
            eprintln!("zeron-zgx: plugin {plugin}: open_tab without \"panel.tabs\"");
            return;
        }
        match surfaces::tab(asked) {
            // (A page's panel only its own plugin's.)
            Ok(tab) if tab.get("page").is_some_and(|p| p["plugin"] != plugin) => {
                eprintln!("zeron-zgx: plugin {plugin}: open_tab on another plugin's page");
            }
            Ok(mut tab) => {
                tab["plugin"] = json!(plugin);
                let name = item.name().to_string();
                self.request("panel_tab", &name, json_value(&tab));
            }
            Err(why) => eprintln!("zeron-zgx: plugin {plugin}: {why}"),
        }
    }

    /// A plugin keeps a value (`zeron.store`): in its storage, at most
    /// `store::MAX_BYTES` in all; `null` lets it go.
    fn store_set(&self, plugin: &str, asked: &Json) {
        let Some(key) = asked["key"].as_str().filter(|k| !k.is_empty() && k.len() <= 200).map(str::to_string) else { return };
        let publish = {
            let mut stores = self.stores.borrow_mut();
            let Some((shared, held)) = stores.get_mut(plugin) else { return };
            let mut next = held.clone();
            match &asked["value"] {
                Json::Null => {
                    next.remove(&key);
                }
                value => {
                    next.insert(key, value.clone());
                }
            }
            if let Err(why) = store::save(plugin, &next) {
                drop(stores);
                let name = self.item(plugin).map(|i| i.name().to_string()).unwrap_or_else(|| plugin.to_string());
                self.toast(plugin, &format!("{name} couldn't keep that: {why}"));
                return;
            }
            *held = next;
            (shared.clone(), json_value(&Json::Object(held.clone())))
        };
        // (Set with no borrow held: the plugin's effects run at once.)
        publish.0.set(publish.1);
    }

    /// The plugins on (`loading`, of `items`): their settings followed, and
    /// published as they change (with no borrow held: plugins' effects and
    /// the sidebar's run at once).
    fn sync_settings(&self, items: &[Item], loading: &HashSet<String>) {
        let on: Vec<settings::On> = items
            .iter()
            .filter(|i| loading.contains(i.id()))
            .map(|i| settings::On {
                id: i.id().to_string(),
                name: i.name().to_string(),
                hash: i.hash().to_string(),
                // Zeron's own, as the engine says (shipped in the app,
                // allowed as it ships, never edited).
                builtin: i.json["builtin"].as_bool() == Some(true),
            })
            .collect();
        let snapshots: HashMap<String, PathBuf> = items
            .iter()
            .filter_map(|i| Some((i.id().to_string(), PathBuf::from(i.json["snapshot"].as_str()?))))
            .collect();
        let changed = self.settings.borrow_mut().sync(&on, |id| snapshots.get(id).and_then(|s| settings::manifest_in(s)));
        self.publish_settings(changed.list, &changed.values);
    }

    fn publish_settings(&self, list: bool, values: &[String]) {
        let (list_value, own): (Option<Value>, Vec<(Shared, Value)>) = {
            let book = self.settings.borrow();
            let own = self.settings_own.borrow();
            (
                (list || !values.is_empty()).then(|| json_value(&book.list())),
                values
                    .iter()
                    .filter_map(|id| Some((own.get(id)?.clone(), json_value(&Json::Object(book.values(id))))))
                    .collect(),
            )
        };
        if let Some(v) = list_value {
            self.settings_list.set(v);
        }
        for (shared, value) in own {
            shared.set(value);
        }
    }

    /// One of a plugin's settings set from its page (null: its default).
    fn setting_set(&self, asked: &Json) -> Result<(), String> {
        let plugin = asked["plugin"].as_str().unwrap_or_default();
        let id = asked["id"].as_str().unwrap_or_default();
        self.settings.borrow_mut().set(plugin, id, &asked["value"])?;
        self.publish_settings(true, &[plugin.to_string()]);
        Ok(())
    }

    /// The sessions a plugin shows ("github"): each one's pull request
    /// watched where its checkout lives, those it no longer shows let go.
    fn github_watch(&self, plugin: &str, list: &Json) {
        let Some(item) = self.item(plugin) else { return };
        if !item.may("github") {
            eprintln!("zeron-zgx: plugin {plugin}: watch_pr without \"github\"");
            return;
        }
        let sessions = value_json(&self.host.shared("sessions", Value::Nil).signal().with_untracked(Value::clone));
        let wanted: Vec<(String, Json)> = list
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .take(MAX_WATCHED)
            .filter_map(|id| {
                let chat = sessions["chats"].as_array()?.iter().find(|c| c["id"] == id)?;
                Some((id.to_string(), github::watch_params(chat)?))
            })
            .collect();
        let me = Rc::downgrade(&self.me());
        let plugin = plugin.to_string();
        self.tasks.spawn(async move {
            let Some(rpc) = engine::get().connection(Duration::from_secs(5)).await else { return };
            let Some(b) = me.upgrade() else { return };
            let publish = {
                let mut all = b.gh.borrow_mut();
                let Some(gh) = all.get_mut(&plugin) else { return };
                let keep: HashSet<&str> = wanted.iter().map(|(id, _)| id.as_str()).collect();
                for (session, key, params) in gh.watches.want(&wanted, Instant::now(), MAX_KEPT_PRS) {
                    let watch = Self::pr_watch(&b, &rpc, &plugin, &session, &key, &params);
                    gh.watches.started(&session, &key, watch);
                }
                // What each session's pull request was last heard as stays
                // while its row is away (Settings, another page): back, its
                // badge is there at once, and the new watch brings it up to
                // date. Only past a bound do the unwatched ones go.
                let kept = gh.prs.len();
                if gh.prs.len() > MAX_KEPT_PRS {
                    gh.prs.retain(|id, _| keep.contains(id.as_str()));
                }
                // (Published only as what's kept changed: rows coming into
                // view as a list scrolls ask this often, and each publish
                // runs every reader of it.)
                if gh.prs.len() == kept {
                    return;
                }
                gh.shared.clone().map(|s| (s, json_value(&Json::Object(gh.prs.clone()))))
            };
            if let Some((shared, value)) = publish {
                shared.set(value);
            }
        });
    }

    /// Session `session`'s pull request watched (`params`, as `key` names
    /// them): what it hears published, and an end told.
    fn pr_watch(b: &Rc<Bridge>, rpc: &Rpc, plugin: &str, session: &str, key: &str, params: &Json) -> Watch {
        let (heard, ended) = (Rc::downgrade(b), Rc::downgrade(b));
        let (plugin, session, key) = (plugin.to_string(), session.to_string(), key.to_string());
        let (p, s) = (plugin.clone(), session.clone());
        rpc.watch_ending(
            "WatchCheckoutChangeRequest",
            params.clone(),
            move |status| {
                if let Some(b) = heard.upgrade() {
                    b.github_heard(&p, &s, &status);
                }
            },
            move |why| {
                if let Some(b) = ended.upgrade() {
                    b.github_ended(&plugin, &session, &key, &why);
                }
            },
        )
    }

    /// A session's pull request watch ended of itself (an error, the
    /// engine gone): started again after its wait, if it's still wanted
    /// as it was. One whose chat isn't there to watch (its folder isn't a
    /// checkout on its device, the device is offline) is set aside instead,
    /// until its chat or device changes (`github_wake`).
    fn github_ended(self: &Rc<Self>, plugin: &str, session: &str, key: &str, why: &str) {
        let ended = {
            let mut all = self.gh.borrow_mut();
            let Some(gh) = all.get_mut(plugin) else { return };
            gh.watches.ended(session, key, why, Instant::now())
        };
        let wait = match ended {
            github::Ended::Stale => return,
            github::Ended::Parked => {
                return eprintln!("zeron-zgx: plugin {plugin}: {session}'s pull request watch ended ({why}); set aside until its chat or device changes");
            }
            github::Ended::Wait(wait) => wait,
        };
        eprintln!("zeron-zgx: plugin {plugin}: {session}'s pull request watch ended ({why}); again in {}s", wait.as_secs());
        let me = Rc::downgrade(self);
        let (plugin, session, key) = (plugin.to_string(), session.to_string(), key.to_string());
        self.tasks.spawn(async move {
            zgx::timer::sleep(wait).await;
            if let Some(b) = me.upgrade() {
                b.github_start(&plugin, &session, &key);
            }
        });
    }

    /// Session `session`'s watch of `key` started again, if it's still
    /// shown as it was, and neither watched nor set aside.
    fn github_start(self: &Rc<Self>, plugin: &str, session: &str, key: &str) {
        let due = |b: &Bridge| b.gh.borrow().get(plugin).and_then(|gh| gh.watches.due(session, key));
        if due(self).is_none() {
            return;
        }
        let me = Rc::downgrade(self);
        let (plugin, session, key) = (plugin.to_string(), session.to_string(), key.to_string());
        self.tasks.spawn(async move {
            let Some(rpc) = engine::get().connection(Duration::from_secs(5)).await else {
                // (No engine yet: waited for again, longer.)
                if let Some(b) = me.upgrade() {
                    b.github_ended(&plugin, &session, &key, "no engine");
                }
                return;
            };
            let Some(b) = me.upgrade() else { return };
            let mut all = b.gh.borrow_mut();
            let Some(gh) = all.get_mut(&plugin) else { return };
            let Some(params) = gh.watches.due(&session, &key) else { return };
            let watch = Self::pr_watch(&b, &rpc, &plugin, &session, &key, &params);
            gh.watches.started(&session, &key, watch);
        });
    }

    /// The session list moved: each set-aside watch whose chat changed
    /// (its folder, branch or device) or whose device came back online is
    /// started again. Nothing is read when none is set aside.
    fn github_wake(self: &Rc<Self>, sessions: &Value) {
        let mut start = Vec::new();
        {
            let mut all = self.gh.borrow_mut();
            for (plugin, gh) in all.iter_mut() {
                if !gh.watches.any_parked() {
                    continue;
                }
                let params = |id: &str| -> Option<Json> {
                    let chats = sessions.field("chats")?.list();
                    let chat = chats.iter().find(|c| c.field("id").map(text).as_deref() == Some(id))?;
                    github::watch_params(&value_json(chat))
                };
                let online = |device: &str| -> Option<bool> {
                    let devices = sessions.field("devices")?.list();
                    let found = devices.iter().find(|d| d.field("id").map(text).as_deref() == Some(device))?;
                    match found.field("online") {
                        Some(Value::Bool(online)) => Some(*online),
                        _ => None,
                    }
                };
                for (session, key, _) in gh.watches.wake(params, online) {
                    start.push((plugin.clone(), session, key));
                }
            }
        }
        for (plugin, session, key) in start {
            self.github_start(&plugin, &session, &key);
        }
    }

    /// A session's pull request, as its watch heard it.
    fn github_heard(&self, plugin: &str, session: &str, status: &Json) {
        let publish = {
            let mut all = self.gh.borrow_mut();
            let Some(gh) = all.get_mut(plugin) else { return };
            if !gh.watches.heard(session) {
                return;
            }
            gh.prs.insert(session.to_string(), github::summary(status));
            gh.shared.clone().map(|s| (s, json_value(&Json::Object(gh.prs.clone()))))
        };
        // (Set with no borrow held: the plugin's effects run at once.)
        if let Some((shared, value)) = publish {
            shared.set(value);
        }
    }

    /// A page of the plugin's own, listed in the sidebar ("pages.add").
    fn add_page(&self, plugin: &str, asked: &Json) {
        let Some(item) = self.item(plugin) else { return };
        if !item.may("pages.add") {
            eprintln!("zeron-zgx: plugin {plugin}: add_page without \"pages.add\"");
            return;
        }
        let page = match surfaces::page(asked) {
            Ok(page) => page,
            Err(why) => return eprintln!("zeron-zgx: plugin {plugin}: {why}"),
        };
        {
            let mut pages = self.pages.borrow_mut();
            let list = pages.entry(plugin.to_string()).or_default();
            let full = list.len() >= MAX_PAGES;
            match list.iter_mut().find(|p| p["id"] == page["id"]) {
                Some(same) if *same == page => return,
                Some(same) => *same = page,
                None if full => return eprintln!("zeron-zgx: plugin {plugin}: at most {MAX_PAGES} pages"),
                None => list.push(page),
            }
        }
        self.publish_pages();
    }

    fn publish_pages(&self) {
        let list: Vec<Value> = self
            .pages
            .borrow()
            .iter()
            .flat_map(|(plugin, pages)| {
                let name = self.item(plugin).map(|i| i.name().to_string()).unwrap_or_else(|| plugin.clone());
                pages.iter().map(move |p| {
                    let mut p = p.clone();
                    p["plugin"] = json!(plugin);
                    p["plugin_name"] = json!(name);
                    json_value(&p)
                }).collect::<Vec<_>>()
            })
            .collect();
        // (Set with no borrow held: the sidebar's effects run at once.)
        self.pages_shared.set(vlist(list));
    }

    /// What the host knows of plugin `id` for `plugin_status`: whether it's
    /// on here, and what its row says (stopped working, not shown in this
    /// template, a node's problem), with its pages.
    fn host_status(&self, plugin: &str) -> Json {
        let on = self.loaded.borrow().iter().any(|p| p.name == plugin);
        let page = value_json(&self.page.signal().with_untracked(Value::clone));
        let row = page["list"].as_array().into_iter().flatten().find(|i| i["id"] == plugin).cloned().unwrap_or(Json::Null);
        let mut out = json!({ "on_in_window": on });
        for key in ["failed", "problem", "hidden"] {
            if !row[key].is_null() {
                out[key] = row[key].clone();
            }
        }
        if let Some(pages) = self.pages.borrow().get(plugin) {
            out["pages"] = json!(pages);
        }
        // Each of its contributions, as zgx counts it now.
        let contributions: Vec<Json> = self
            .host
            .contributions()
            .into_iter()
            .filter(|c| c.plugin == plugin)
            .map(|c| json!({ "at": c.target, "how": c.place, "matches": c.matches, "error": c.error }))
            .collect();
        out["contributions"] = json!(contributions);
        if let Some(cost) = self.costs.borrow().get(plugin) {
            out["cost"] = cost.clone();
            if let Some(slow) = surfaces::cost_words(cost) {
                out["slow"] = json!(slow);
            }
        }
        out
    }

    /// An agent's `plugin_status`, through the engine: the host's part at
    /// once if the plugin isn't running here, else the plugin's own account
    /// of where its nodes are (its SDK's, a "where" call), finished in
    /// `result`.
    fn status_call(&self, plugin: &str, id: String, snapshot: bool) {
        if !self.loaded.borrow().iter().any(|p| p.name == plugin) {
            answer_tool(id, true, self.host_status(plugin), String::new());
            return;
        }
        self.push_call(plugin, id, "where", vec![("snapshot", Value::from(snapshot))]);
    }

    /// The plugin's account of where it is, with the host's, and a picture
    /// of the window cut to it if asked; then to the agent.
    fn status_answer(&self, plugin: &str, id: String, call: &Value, answer: &Value) {
        let mut out = self.host_status(plugin);
        let placements = answer.field("value").map(value_json).unwrap_or(Json::Null);
        out["placements"] = placements["placements"].clone();
        let snapshot = call.field("snapshot").is_some_and(Value::truthy);
        if !snapshot {
            answer_tool(id, true, out, String::new());
            return;
        }
        // (A picture after this turn, as the window last drew it.)
        let me = Rc::downgrade(&self.me());
        let plugin = plugin.to_string();
        self.tasks.spawn(async move {
            let Some(b) = me.upgrade() else { return };
            match b.picture(&plugin, &out["placements"]) {
                Ok(said) => {
                    for (k, v) in said.as_object().into_iter().flatten() {
                        out[k] = v.clone();
                    }
                }
                Err(why) => out["snapshot_error"] = json!(why),
            }
            answer_tool(id, true, out, String::new());
        });
    }

    /// The window as it shows, cut to around `placements`' regions, saved
    /// for the agent to look at; none when none of its nodes shows (never
    /// the whole window: what else the window shows isn't the plugin's to
    /// hand an agent). The regions are the plugin's own account of where
    /// its nodes are, until zgx measures a plugin's nodes for the host
    /// (asked of zgx): each is held inside the window, and so is the cut.
    fn picture(&self, plugin: &str, placements: &Json) -> Result<Json, String> {
        let ui = self.ui.upgrade().ok_or("The window is closed")?;
        let root = ui.root();
        let image = ui.snapshot(root, SNAPSHOT_WIDTH).ok_or("The window isn't showing")?;
        let bounds = ui.scene.borrow().bounds(root);
        if bounds.width <= 0. {
            return Err("The window isn't laid out".into());
        }
        let scale = image.width() as f32 / bounds.width;
        let found = surfaces::regions(placements);
        let region = surfaces::around(&found, 24., bounds.width, bounds.height)
            .ok_or("None of its nodes shows now, so there's nothing of it to picture")?;
        let (w, h, pixels) = surfaces::crop(image.pixels(), image.width(), image.height(), region, scale);
        let dir = engine::data_dir().ok_or("Zeron's data folder isn't known")?.join("plugins").join(".snapshots");
        let path = dir.join(format!("{plugin}.png"));
        surfaces::save_png(&path, w, h, &pixels)?;
        Ok(json!({
            "snapshot": path.display().to_string(),
            "snapshot_region": { "x": region.x, "y": region.y, "w": region.w, "h": region.h },
            "snapshot_note": "The window around where its nodes show.",
        }))
    }

    fn set_menus(&self, plugin: &str, list: &Value) {
        let Some(item) = self.item(plugin) else { return };
        if !item.may("ui.add") {
            return;
        }
        let items: Vec<Value> = list
            .list()
            .iter()
            .filter_map(|v| {
                let kind = v.field("kind").map(text)?;
                let id = v.field("id").map(text)?;
                let label: String = v.field("label").map(text)?.chars().take(LABEL).collect();
                (plugin_menus::valid_kind(&kind) && !id.is_empty() && !label.trim().is_empty()).then(|| {
                    Value::record([
                        ("plugin", Value::from(plugin)),
                        ("plugin_name", Value::from(item.name())),
                        ("kind", Value::from(kind.as_str())),
                        ("id", Value::from(id.as_str())),
                        ("label", Value::from(label.as_str())),
                    ])
                })
            })
            .take(20)
            .collect();
        self.item_menus.borrow_mut().insert(plugin.into(), items);
        self.publish_menus();
    }

    fn toast(&self, plugin: &str, message: &str) {
        let Some(item) = self.item(plugin) else { return };
        // One a second, each plugin.
        let now = Instant::now();
        if let Some(at) = self.toasted.borrow().get(plugin)
            && now.duration_since(*at) < Duration::from_secs(1)
        {
            return;
        }
        self.toasted.borrow_mut().insert(plugin.into(), now);
        let message: String = message.chars().take(200).collect();
        self.request("toast", item.name(), Value::from(message.as_str()));
    }

    /// The main window's actions (its scripts', never a plugin's: none is
    /// exposed but `zeron.open` and `zeron.compose.insert`).
    fn actions(&self) {
        let host = &self.host;
        let me = Rc::downgrade(&self.me());
        // Which device's plugins Settings → Plugins shows: another's (its
        // id), or this one's (nil). The same one again looks again.
        host.action("plugins.target", {
            let me = me.clone();
            move |id| {
                if let Some(b) = me.upgrade() {
                    let id = text(&id);
                    remote::retarget(&b, (!id.is_empty()).then_some(id));
                }
            }
        });
        // `id`, or `{ id, device?, device_name? }`: its sheet, for the device
        // it's on (named on the sheet when it isn't this one).
        host.action("plugins.review", {
            let me = me.clone();
            move |arg| {
                if let Some(b) = me.upgrade() {
                    let (id, device) = remote::id_and_device(&arg);
                    let name = arg.field("device_name").map(text).unwrap_or_default();
                    consent::review(&b, &id, device.map(|d| (d, name)));
                }
            }
        });
        // (Each of these acts on the device it names, `device`, or this one.)
        set_off_action(host, me.clone());
        host.async_action("plugins.remove", {
            let me = me.clone();
            move |arg| {
                let me = me.clone();
                async move { crate::plugins::preview::refuse("plugins.remove")?;
                    let (id, device) = remote::id_and_device(&arg);
                    let elsewhere = device.is_some();
                    call_on(&me, device, "RemovePlugin", json!({ "id": id })).await?;
                    // Its settings go with it (they're this device's: another
                    // device's removal leaves them).
                    if elsewhere {
                    } else if let Some(b) = me.upgrade() {
                        b.settings.borrow_mut().forget(&id);
                    } else if let Some(dir) = settings::storage_dir() {
                        settings::forget_in(&dir, &id);
                    }
                    Ok(Value::Nil)
                }
            }
        });
        // A plugin's setting, set from its page in Settings (never by a
        // plugin: this action is the app's alone).
        host.action("plugins.setting_set", {
            let me = me.clone();
            move |asked| {
                if crate::plugins::preview::refuse("plugins.setting_set").is_err() {
                    return;
                }
                if let Some(b) = me.upgrade()
                    && let Err(why) = b.setting_set(&value_json(&asked))
                {
                    eprintln!("zeron-zgx: plugins: a setting wasn't set: {why}");
                }
            }
        });
        // A folder setting's folder: the window's dialog, the folder chosen
        // set as its value (its path); a promise of it, or nil.
        host.async_action("plugins.setting_folder", {
            let me = me.clone();
            move |asked| {
                let me = me.clone();
                async move {
                    let asked = value_json(&asked);
                    let dialogs = me.upgrade().map(|b| b.dialogs.clone()).ok_or("The window closed")?;
                    let title = asked["title"].as_str().filter(|t| !t.is_empty()).unwrap_or("Choose a Folder").to_string();
                    let chosen = dialogs.open_folder(zgx_desktop::FileDialogOptions::new(title)).await.map_err(|e| e.to_string())?;
                    let Some(path) = chosen else { return Ok(Value::Nil) };
                    let path = path.to_string_lossy().to_string();
                    let b = me.upgrade().ok_or("The window closed")?;
                    b.setting_set(&json!({ "plugin": asked["plugin"], "id": asked["id"], "value": path }))?;
                    Ok(Value::from(path.as_str()))
                }
            }
        });
        // Someone else's plugin, from a .zeron-plugin file or a link (a
        // package, a GitHub release, a git repository). The engine verifies
        // it first; anyone else's waits for the consent window. A file here
        // installed on another device is sent there (remote::send_file).
        host.async_action("plugins.install", {
            let me = me.clone();
            move |arg| {
                let me = me.clone();
                async move { crate::plugins::preview::refuse("plugins.install")?;
                    let given = |name: &str| arg.field(name).map(text).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                    let device = remote::device_of(&arg);
                    let params = match (given("path"), given("url")) {
                        (Some(path), _) if device.is_some() => {
                            let upload = remote::send_file(&path, |piece| call_on(&me, device.clone(), "StagePluginPackage", piece)).await?;
                            remote::install_sent(&path, &upload)
                        }
                        (Some(path), _) => json!({ "path": path }),
                        (None, Some(url)) => json!({ "url": url }),
                        _ => return Err("Choose a plugin file or paste a link".into()),
                    };
                    let reply = call_on(&me, device, "InstallPlugin", params).await?;
                    eprintln!("zeron-zgx: plugins: installed {}", reply["plugin"]["id"].as_str().unwrap_or("?"));
                    Ok(json_value(&reply))
                }
            }
        });
        // The update found for it (what it changes in its grants is on its
        // item); one asking for more waits for the consent window again.
        host.async_action("plugins.update", {
            let me = me.clone();
            move |arg| {
                let me = me.clone();
                async move { crate::plugins::preview::refuse("plugins.update")?;
                    let (id, device) = remote::id_and_device(&arg);
                    let reply = call_on(&me, device, "UpdatePlugin", json!({ "id": id })).await?;
                    Ok(json_value(&reply))
                }
            }
        });
        // Every installed plugin's source checked for a new version (nil, or
        // `{ device }`).
        host.async_action("plugins.check_updates", {
            let me = me.clone();
            move |arg| {
                let me = me.clone();
                async move {
                    let reply = call_on(&me, remote::device_of(&arg), "CheckPluginUpdates", json!({})).await?;
                    Ok(json_value(&reply))
                }
            }
        });
        host.async_action("plugins.new", |name| async move { crate::plugins::preview::refuse("plugins.new")?;
            let name = text(&name).trim().to_string();
            let id = slug(&name);
            if id.len() < 2 {
                return Err("Give it a name of a few letters".into());
            }
            // A new plugin asks for nothing: what it needs, you allow later.
            let item = call("NewPlugin", json!({ "id": id, "name": name, "permissions": [] })).await?;
            Ok(Value::from(item["id"].as_str().unwrap_or(&id)))
        });
        host.action("plugins.open_folder", {
            let me = me.clone();
            move |id| {
                let Some(b) = me.upgrade() else { return };
                let dir = match &id {
                    Value::Str(id) => b.item(id).and_then(|i| i.json["dir"].as_str().map(PathBuf::from)),
                    _ => b.dirs.borrow().as_ref().map(|(p, _)| p.clone()),
                };
                if let Some(dir) = dir {
                    let _ = std::fs::create_dir_all(&dir);
                    reveal(&dir);
                }
            }
        });
        host.action("plugins.menu", {
            let me = me.clone();
            move |arg| {
                let Some(b) = me.upgrade() else { return };
                let plugin = arg.field("plugin").map(text).unwrap_or_default();
                let item = arg.field("id").map(text).unwrap_or_default();
                if !b.loaded.borrow().iter().any(|p| p.name == plugin) {
                    return;
                }
                let id = format!("menu-{}", next_id());
                let context = arg.field("context").cloned().unwrap_or(Value::Nil);
                b.push_call(&plugin, id, "menu", vec![("item", Value::from(item.as_str())), ("args", context)]);
            }
        });
        // Templates (screens/templates): switching, duplicating and deleting
        // are the engine's, as an agent's use_template and the rest are.
        host.async_action("templates.use", {
            let me = me.clone();
            move |id| {
                let me = me.clone();
                async move {
                    // (A preview can't switch templates: preview.rs.)
                    preview::refuse("templates.use")?;
                    template_call("UseTemplate", json!({ "id": text(&id) })).await?;
                    // Chosen here, on Zeron's own Templates page (never a
                    // template's: screens/templates is never templated):
                    // out of safe mode, and no notice of the switch.
                    preview::leave_safe();
                    if let Some(b) = me.upgrade() {
                        b.chose.set(true);
                        b.look();
                        b.rewatch();
                    }
                    Ok(Value::Nil)
                }
            }
        });
        // A new template by the name it's given: a copy of Zeron's own.
        host.async_action("templates.create", {
            let me = me.clone();
            move |name| {
                let me = me.clone();
                async move {
                    preview::refuse("templates.create")?;
                    let b = me.upgrade().ok_or("closed")?;
                    let root = b.templates_dir().ok_or("Templates aren't available right now")?;
                    let name = text(&name).trim().chars().take(60).collect::<String>();
                    let new_id = live::new_id(&root, &name);
                    template_call("CreateTemplate", json!({ "id": new_id, "name": name, "from": live::BUILTIN })).await?;
                    b.look();
                    b.rewatch();
                    Ok(Value::from(new_id.as_str()))
                }
            }
        });
        host.async_action("templates.duplicate", {
            let me = me.clone();
            move |id| {
                let me = me.clone();
                async move {
                    preview::refuse("templates.duplicate")?;
                    let b = me.upgrade().ok_or("closed")?;
                    let root = b.templates_dir().ok_or("Templates aren't available right now")?;
                    let from = text(&id);
                    let (new_id, name) = live::copy_name(&root, &from);
                    template_call("CreateTemplate", json!({ "id": new_id, "name": name, "from": from })).await?;
                    b.look();
                    b.rewatch();
                    Ok(Value::from(new_id.as_str()))
                }
            }
        });
        host.async_action("templates.delete", {
            let me = me.clone();
            move |id| {
                let me = me.clone();
                async move {
                    preview::refuse("templates.delete")?;
                    template_call("DeleteTemplate", json!({ "id": text(&id) })).await?;
                    // One made later under its id isn't one you kept.
                    if let Some(path) = preview::approvals_path() {
                        preview::forget_at(&path, &text(&id));
                    }
                    if let Some(b) = me.upgrade() {
                        b.look();
                        b.rewatch();
                    }
                    Ok(Value::Nil)
                }
            }
        });
        // Sharing (screens/templates): a template signed as yours, as one
        // file; and one brought in. Yours installs; someone else's waits for
        // you in its own consent window (template_consent), which alone can
        // allow it.
        host.async_action("templates.export", |arg| async move {
            preview::refuse("templates.export")?;
            let id = arg.field("id").map(text).unwrap_or_default();
            let mut path = arg.field("path").map(text).unwrap_or_default();
            // (A save dialog's name, its extension taken off.)
            if !path.is_empty() && !path.ends_with(".zeron-template") {
                path.push_str(".zeron-template");
            }
            template_call("ExportTemplate", json!({ "id": id, "path": path })).await.map(|v| json_value(&v))
        });
        host.async_action("templates.import", {
            let me = me.clone();
            // A file (`path`, or `{ path }`) or a link (`{ url }`).
            move |arg| {
                let me = me.clone();
                async move {
                    preview::refuse("templates.import")?;
                    let given = |name: &str| arg.field(name).map(text).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                    let params = match (given("path").or_else(|| Some(text(&arg)).filter(|s| !s.is_empty())), given("url")) {
                        (Some(path), None) => json!({ "path": path }),
                        (None, Some(url)) => json!({ "url": url }),
                        _ => return Err("Choose a template file or paste a link".into()),
                    };
                    let reply = template_call("ImportTemplate", params).await?;
                    if let Some(b) = me.upgrade() {
                        if let Some(name) = reply["pending"].as_str() {
                            // Shown now: no "waiting" toast for it as well.
                            b.told.borrow_mut().insert(format!("template:{name}"));
                            template_consent::review(&b, name);
                        }
                        b.look();
                    }
                    Ok(json_value(&reply))
                }
            }
        });
        // Each template from a link (or a file) checked where it came from
        // for a newer version; and the one found for a template, installed
        // in its place.
        host.async_action("templates.check_updates", {
            let me = me.clone();
            move |_| {
                let me = me.clone();
                async move {
                    let reply = template_call("CheckTemplateUpdates", json!({})).await?;
                    if let Some(b) = me.upgrade() {
                        b.look();
                    }
                    Ok(json_value(&reply))
                }
            }
        });
        host.async_action("templates.update", {
            let me = me.clone();
            move |id| {
                let me = me.clone();
                async move {
                    preview::refuse("templates.update")?;
                    let reply = template_call("UpdateTemplate", json!({ "id": text(&id) })).await?;
                    if let Some(b) = me.upgrade() {
                        // A new version with new code waits for you, as an
                        // import does: in Zeron's own consent window.
                        if let Some(name) = reply["pending"].as_str() {
                            b.told.borrow_mut().insert(format!("template:{name}"));
                            template_consent::review(&b, name);
                        }
                        b.look();
                    }
                    Ok(json_value(&reply))
                }
            }
        });
        host.action("templates.review", {
            let me = me.clone();
            move |name| {
                if let Some(b) = me.upgrade() {
                    template_consent::review(&b, &text(&name));
                }
            }
        });
        host.action("templates.open_folder", {
            let me = me.clone();
            move |id| {
                let Some(b) = me.upgrade() else { return };
                let Some(root) = b.templates_dir() else { return };
                let id = text(&id);
                // No id: the templates folder itself (the page's empty state).
                if id.is_empty() {
                    if root.is_dir() {
                        reveal(&root);
                    }
                    return;
                }
                let dir = if id == live::BUILTIN { root.join(live::ZERON) } else { root.join(&id) };
                if live::plain(&id) && dir.is_dir() {
                    reveal(&dir);
                }
            }
        });
        // A screen that failed as it built (the shell's boundary): blamed on
        // the file its error names, after the build.
        host.action("templates.failed", {
            let me = me.clone();
            move |arg| {
                if let Some(b) = me.upgrade() {
                    let error = arg.field("error").map(text).unwrap_or_default();
                    let me = Rc::downgrade(&b);
                    b.tasks.spawn(async move {
                        if let Some(b) = me.upgrade() {
                            b.blame(&error);
                        }
                    });
                }
            }
        });
        // What plugins may ask of the app, through screens/plugins.
        host.action("zeron.open", {
            let me = me.clone();
            move |to| {
                if let Some(b) = me.upgrade() {
                    b.request("open", "", to);
                }
            }
        });
        host.action("zeron.compose.insert", {
            let me = me.clone();
            move |text_| {
                if let Some(b) = me.upgrade() {
                    b.request("compose", "", Value::from(text(&text_).as_str()));
                }
            }
        });
    }
}

/// A Luau list of `items`.
pub(crate) fn vlist(items: Vec<Value>) -> Value {
    Value::List(items.into())
}


fn as_f64(value: &Value) -> Option<f64> {
    value.to_json().parse::<f64>().ok()
}

fn next_id() -> u64 {
    thread_local!(static NEXT: Cell<u64> = const { Cell::new(0) });
    NEXT.with(|n| {
        n.set(n.get() + 1);
        n.get()
    })
}

/// A plugin id from a name: "My Plugin!" → "my-plugin".
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.trim_end_matches('-').chars().take(40).collect();
    out.trim_end_matches('-').to_string()
}

/// A template call: asked again once the account's setup is finished if
/// the engine answered `setup_incomplete` (no organization chosen yet), so
/// the templates page works as soon as setup does.
async fn template_call(method: &str, params: Json) -> Result<Json, String> {
    crate::auth::after_setup(|| call(method, params.clone())).await
}

async fn call(method: &str, params: Json) -> Result<Json, String> {
    engine::get().call(method, params).await.map_err(|e| {
        // The engine's own words where it gave some; "offline" as the app says it.
        if e == "offline" || e == "closed" { engine::outcome(&e).to_string() } else { e }
    })
}

/// `call`, on `device` (none: this one). A device out of reach says
/// "offline", and the page showing it says so too.
#[cfg(test)]
pub(crate) fn set_off_action_for_tests(host: &ScriptHost) {
    set_off_action(host, Weak::new());
}

/// `plugins.set_off`: a plugin off, or back on. Never an unkept preview's.
fn set_off_action(host: &ScriptHost, me: Weak<Bridge>) {
    host.async_action("plugins.set_off", move |arg| {
        let me = me.clone();
        async move {
            preview::refuse("plugins.set_off")?;
            let (id, device) = remote::id_and_device(&arg);
            let off = arg.field("off").is_some_and(Value::truthy);
            eprintln!("zeron-zgx: plugins: {id} turned {} from Settings", if off { "off" } else { "on" });
            call_on(&me, device, "SetPluginOff", json!({ "id": id, "off": off })).await.map(|_| Value::Nil)
        }
    });
}

async fn call_on(me: &Weak<Bridge>, device: Option<String>, method: &str, params: Json) -> Result<Json, String> {
    let Some(device) = device else { return call(method, params).await };
    call(method, remote::addressed(Some(&device), params)).await.map_err(|error| {
        if !remote::unreachable(&error) {
            return error;
        }
        if let Some(b) = me.upgrade() {
            b.remote.failed(&device, &error);
        }
        "offline".to_string()
    })
}

/// At most this many pages a plugin.
const MAX_PAGES: usize = 8;
/// At most this many sessions' pull requests watched for a plugin.
const MAX_WATCHED: usize = 200;
/// Sessions whose last-heard pull request is kept while unwatched (above
/// this, only the watched are kept).
const MAX_KEPT_PRS: usize = 512;
/// A status picture's width at most, in pixels.
const SNAPSHOT_WIDTH: u32 = 2400;

fn answer_tool(id: String, ok: bool, value: Json, error: String) {
    engine::get().spawn(async move {
        let mut params = json!({ "callId": id, "ok": ok });
        if ok {
            params["value"] = value;
        } else {
            params["error"] = Json::String(error);
        }
        let said = engine::get().call("PluginToolResult", params).await;
        if std::env::var_os("ZERON_ZGX_PLUGIN_LOG").is_some() {
            eprintln!("zeron-zgx: plugins: the answer went: {said:?}");
        }
        if let Err(e) = said {
            eprintln!("zeron-zgx: a tool's answer didn't reach the agent: {e}");
        }
    });
}

fn reveal(dir: &Path) {
    if let Err(error) = open::that_detached(dir) {
        eprintln!("zeron-zgx: the folder didn't open: {error}");
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    fn item(json: Json) -> Item {
        Item { json }
    }

    /// R26: a plugin removed, or back under its id from somewhere else,
    /// starts afresh; one updated from where it came keeps what it had.
    #[test]
    fn a_plugin_removed_or_put_back_from_elsewhere_is_forgotten() {
        let at = |id: &str, url: &str| item(json!({ "id": id, "source": { "kind": "git", "url": url } }));
        let before = vec![at("focus", "https://a/focus"), at("clock", "https://a/clock"), at("ping", "https://a/ping")];
        let now = vec![at("focus", "https://a/focus"), at("ping", "https://evil/ping")];
        let gone: Vec<String> = gone(&before, &now).into_iter().map(|(id, _)| id).collect();
        assert_eq!(gone, vec!["clock".to_string(), "ping".to_string()]);
    }

    /// R21: a plugin's review comment goes on the chat shown alone, with
    /// at most the 40 lines of code that are sent (a 20,000-character
    /// snippet once rode along unseen, onto any chat's draft).
    #[test]
    fn a_review_comment_goes_on_the_chat_shown_with_at_most_40_lines() {
        let snippet: String = (1..=100).map(|n| format!("line {n}\n")).collect();
        let asked = json_value(&json!({ "file": "a.rs", "line": 1, "note": "x".repeat(9000), "snippet": snippet }));
        let shown = comment(&asked, "s1").expect("on the chat shown");
        let kept = text(shown.field("snippet").unwrap());
        assert_eq!(kept.lines().count(), 40, "{kept}");
        assert!(kept.ends_with("line 40"));
        assert_eq!(text(shown.field("note").unwrap()).len(), COMMENT_NOTE);
        assert_eq!(text(shown.field("session").unwrap()), "s1");
        let elsewhere = json_value(&json!({ "file": "a.rs", "line": 1, "note": "n", "session": "s2" }));
        assert!(comment(&elsewhere, "s1").is_none(), "another chat's draft");
        assert!(comment(&asked, "").is_none(), "no chat shown");
        let wide = json_value(&json!({ "file": "a.rs", "line": 1, "note": "n", "snippet": "é".repeat(3000) }));
        assert!(text(comment(&wide, "s1").unwrap().field("snippet").unwrap()).len() <= COMMENT_BYTES);
    }

    #[test]
    fn only_a_plugin_on_loads_from_its_snapshot_with_its_grants() {
        let on = item(json!({
            "id": "focus", "name": "Focus", "status": "on", "entry": "init.luau",
            "snapshot": "/d/plugins/.loaded/focus/0123", "permissions": ["ui.add", "ui.change"],
            "ui": { "change": ["#sidebar-actions"] }
        }));
        let p = on.plugin().unwrap();
        assert_eq!(p.name, "focus");
        assert_eq!(p.entry, PathBuf::from("/d/plugins/.loaded/focus/0123/init.luau"));
        assert!(p.grants.contains(&Capability::OverrideElement("#sidebar-actions".into())));
        for status in ["off", "pending", "tampered", "broken"] {
            let mut json = on.json.clone();
            json["status"] = json!(status);
            assert!(item(json).plugin().is_none(), "{status} must not load");
        }
    }

    /// B10 (R26): the sidebar's notice names the plugin allowed to narrow
    /// it, as installed, never what the plugin writes in `by`.
    #[test]
    fn the_sidebar_filter_is_named_by_the_one_plugin_allowed_to_set_it() {
        let at = |id: &str, name: &str, change: &[&str]| {
            item(json!({ "id": id, "name": name, "status": "on", "snapshot": "/s", "permissions": ["ui.change"], "ui": { "change": change } }))
        };
        let focus = at("focus", "Focus", &["#sidebar"]);
        let badge = at("badge", "Badge", &["#bar"]);
        let quiet = at("quiet", "Quiet", &[".session-row"]);
        let want = |items: &[&Item]| items.iter().filter_map(|i| i.plugin()).collect::<Vec<_>>();
        let items = vec![focus.clone(), badge.clone(), quiet.clone()];
        assert_eq!(filter_by(&want(&[&focus, &badge]), &items), Value::from("Focus"));
        assert_eq!(filter_by(&want(&[&badge]), &items), Value::Nil, "none allowed");
        assert_eq!(filter_by(&want(&[&focus, &quiet]), &items), Value::Nil, "two allowed: which one isn't known");
    }

    #[test]
    fn an_entry_outside_its_snapshot_or_a_reserved_id_never_loads() {
        let base = json!({ "id": "x", "status": "on", "snapshot": "/s", "entry": "../../evil.luau" });
        assert!(item(base.clone()).plugin().is_none());
        let mut reserved = base;
        reserved["entry"] = json!("init.luau");
        reserved["id"] = json!("templates");
        assert!(item(reserved).plugin().is_none());
    }

    #[test]
    fn names_make_ids() {
        assert_eq!(slug("My Plugin!"), "my-plugin");
        assert_eq!(slug("  Ping  pong "), "ping-pong");
        assert_eq!(slug("Ünïcode ok"), "n-code-ok");
    }
}

#[cfg(test)]
mod loading_tests;
#[cfg(test)]
mod examples_tests;
#[cfg(test)]
mod hotkeys_tests;
#[cfg(test)]
mod markdown_tests;
#[cfg(test)]
mod network_tests;

#[cfg(test)]
mod menu_tests {
    /// "Use Zeron v3" is the host's own, in View, whatever the
    /// scripts declare (none here, as when a template broke before any).
    #[test]
    fn the_host_s_menu_always_has_a_way_back_to_zeron_s_template() {
        let menus = format!("{:?}", crate::menu_bar::Bar::of(&[], &[], &[], None).menus);
        assert!(menus.contains("View"), "{menus}");
        assert!(menus.contains("Use Zeron v3"), "{menus}");
        assert!(menus.contains(super::USE_ZERON_TEMPLATE), "{menus}");
    }

    fn command(id: &str, title: &str, menu: &str) -> zgx_script::Command {
        zgx_script::Command { id: id.into(), title: title.into(), menu: menu.into(), key: None, separator: false }
    }

    /// R12: a template that registers the host's own action (or any
    /// `zeron.` id) can't take the menu bar away: the platform refuses a
    /// whole bar with an action twice, so the host leaves the script's out,
    /// and the bar it sets is one the platform takes, with one way back.
    #[test]
    fn a_template_registering_use_zeron_template_keeps_the_menu_bar() {
        let commands = vec![
            command(super::USE_ZERON_TEMPLATE, "Stay Here", "View"),
            command(super::KEEP_TEMPLATE, "Keep Me", "View"),
            command("zeron.anything", "Mine", "File"),
            command("new-chat", "New Chat", "File"),
        ];
        let bar = crate::menu_bar::Bar::of(&commands, &[], &[], None).menus;
        let shown = format!("{bar:?}");
        assert!(zgx_desktop::app_menu_bar(bar, |_| {}).is_ok(), "the platform takes the bar: {shown}");
        assert_eq!(shown.matches(super::USE_ZERON_TEMPLATE).count(), 1, "{shown}");
        assert!(shown.contains("Use Zeron v3") && !shown.contains("Stay Here") && !shown.contains("Keep Me"), "{shown}");
        assert!(!shown.contains("zeron.anything") && shown.contains("New Chat"), "{shown}");
    }
}
