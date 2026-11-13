//! The coding agents on this Mac, for the scripts: what the engine's
//! harness registry finds, each agent's accounts, and signing into one, as
//! the shipping app's Settings → Providers does it (crates/ui/src/settings/
//! harnesses.rs and accounts.rs).
//!
//! What scripts see, `agents`:
//!   { state = "looking" | "ready" | "unavailable",
//!     list = { { id, name, mark?, installed, can_install, enabled,
//!                version?, installing, login = "idle" | "waiting" | "failed",
//!                login_code? (a device code to check on the page opened),
//!                signed_in?, signed_in_as?, can_sign_out, signing_out,
//!                login_methods = { { id, name, description?, kind, var?,
//!                link? } }
//!                (an agent from the ACP registry, as it says it is),
//!                accounts = { { id, email, plan, session, week, active } } } } }
//! Each agent carries all it shows, so a page keys its rows by `id` and an
//! account by its `id`, and they update in place. (`session` and `week` are
//! the plan's two windows, 0 to 1; `mark` is the design system's name for
//! the agent's mark, nil for one it has none for.)
//! and call, each rejecting with an outcome word as its `message`:
//!   agents.load (an action)        looks again (and once at first)
//!   agents.sign_in(id)             a new account for that agent: the browser
//!                                  opened on its sign-in, settled once the
//!                                  account is there; cancelled, failed
//!   agents.cancel(id) (an action)  gives up that sign-in
//!   agents.sign_in({ id, method }) the same, by the method an agent from the
//!                                  ACP registry offers (`login_methods`)
//!   agents.set_key({ id, var, value })  a key such an agent reads from its
//!                                  environment (a `login_methods` kind
//!                                  "env_var"), kept there; failed
//!   agents.sign_out(id)            signs an agent from the ACP registry out
//!                                  (its own `logout`); failed
//!   agents.complete({ id, code })  finishes a sign-in that asked for a code
//!                                  pasted back (its `login` "code": Claude's,
//!                                  when no loopback port was free)
//!   agents.dismiss(id) (an action) forgets a sign-in that failed
//!   agents.set_enabled({ id, enabled })
//!   agents.switch({ id, account })   that account in use from now on
//!   agents.remove({ id, account })   that account forgotten
//!   agents.install(id)             the agent installed (minutes); failed
//!   agents.cancel_install(id) (an action)
//!   agents.use_reset({ account, credit })  spends one of a Codex account's
//!                                  banked resets (`ConsumeCodexResetCredit`,
//!                                  on the device shown): resolves with
//!                                  reset, nothing or gone; rejects with
//!                                  mismatch, signed_out, missing, busy,
//!                                  cancelled or failed
//!   agents.dismiss_reset(account) (an action) forgets its last word
//! Any can also reject with offline.
//!
//! A Codex account also carries what it has banked (`banked`, a count, and
//! `resets`, soonest to expire first: `{ id, title, scope?, expires? }`) and
//! its last reset's word (`reset = { state, why? }`: "using", "reset" for
//! five minutes, "nothing", "gone" or "failed", the last three until
//! dismissed). Until a fresh usage probe lands after a reset, the card shows
//! what it did: the windows at 0 and that reset gone from the count.
//!
//! A sign-in some agents only finish by a code pasted back (Claude's, when
//! no loopback port can be had) isn't offered here: it ends `failed`, and
//! Settings is where to finish it.

use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap},
    rc::Rc,
    time::{Duration, Instant},
};

use serde_json::{Value as Json, json};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::auth::text;
use crate::engine::{self, rpc::Watch};

/// What the engine says before the command of a sign-in that needs typed
/// input (agent_accounts/registry_login.rs).
const TERMINAL_COMMAND: &str = " Run it there: ";

/// How often a sign-in under way is asked after (the shipping app's).
const POLL: Duration = Duration::from_millis(1500);
/// The engine gives a sign-in up after 15 minutes.
const SIGN_IN_WAIT: Duration = Duration::from_secs(15 * 60);

type Outcome = &'static str;

#[derive(Clone, Copy, PartialEq)]
enum Look {
    Looking,
    Ready,
    Unavailable,
}

#[derive(Default)]
struct Login {
    /// Moves on every start and cancel: an older poll stops.
    generation: u64,
    failed: bool,
    waiting: bool,
    /// Waiting for a code pasted back (`CompleteAgentLogin`), not the
    /// browser.
    code: bool,
    /// The engine's id for the sign-in under way.
    login_id: Option<String>,
    /// The device it's under way on (none: this Mac's): asked after and
    /// cancelled there, whichever device the page shows since.
    target: Option<String>,
    /// What the sign-in says to do (a device code to type in), or why it
    /// failed, as the engine words it.
    note: Option<String>,
    /// A sign-in that needs typed input: the command to run in a terminal
    /// on the agent's device.
    command: Option<String>,
    /// The page the sign-in opened (to open again from the card).
    page: Option<String>,
    /// A device code it showed, to check against that page.
    pairing: Option<String>,
}

/// An agent from the ACP registry's sign-in, as it says it is
/// (`GetAcpAgentSignIn`, on the device shown).
#[derive(Default, Clone)]
struct SignIn {
    signed_in: bool,
    /// Who as, when it says.
    account: Option<String>,
    can_sign_out: bool,
    signing_out: bool,
    /// The ways it offers to sign in (`initialize`'s `authMethods`, each with
    /// its kind: agent, terminal or env_var).
    methods: Vec<Json>,
}

struct Agents {
    shared: RefCell<Vec<Shared>>,
    look: Cell<Look>,
    /// The registry's descriptors, as the engine sent them.
    harnesses: RefCell<Vec<Json>>,
    /// Accounts by harness.
    accounts: RefCell<BTreeMap<String, Vec<Json>>>,
    /// Installed versions by harness (`WatchHarnessUpdates`).
    versions: RefCell<HashMap<String, String>>,
    /// Each agent's updates, as the same watch says: a newer version ready
    /// to take (`latest`), its policy, and whether one is being taken.
    update_status: RefCell<HashMap<String, Update>>,
    /// Whose agents these are: another of the account's devices (its id),
    /// or this Mac's (none). Every call carries it as `targetDeviceId`, and
    /// the engine forwards it there, sign-ins' browser callbacks included.
    target: RefCell<Option<String>>,
    /// Each device's accounts as a composer on it asked for them, by device
    /// id ("": this Mac's), whichever device Settings shows
    /// (`agents.on_device`).
    device_accounts: RefCell<BTreeMap<String, DeviceAccounts>>,
    device_shared: RefCell<Vec<Shared>>,
    installing: RefCell<Vec<String>>,
    logins: RefCell<HashMap<String, Login>>,
    /// Registry agents' sign-ins, by harness, once they've said.
    sign_ins: RefCell<HashMap<String, SignIn>>,
    updates: RefCell<Option<Watch>>,
    /// Asked for at least once: a new connection looks again.
    wanted: Cell<bool>,
    loading: Cell<u64>,
    /// Each account's last banked reset (Codex), by account id.
    resets: RefCell<HashMap<String, Reset>>,
    /// The idempotency key each (account, credit) is redeemed with, kept
    /// until Codex gives a definite answer: a retry after a failure is the
    /// same request, never a second reset.
    reset_keys: RefCell<HashMap<(String, String), String>>,
    reset_generation: Cell<u64>,
}

/// How long "the limits started over" stays on the row.
const RESET_SHOWN: Duration = Duration::from_secs(5 * 60);

/// A banked reset being used, or how its use went.
#[derive(Clone)]
struct Reset {
    /// "using", "reset", "nothing" (no window needed it: still banked),
    /// "gone" (no longer available) or "failed".
    state: &'static str,
    /// Why it failed: mismatch, signed_out, missing, offline or failed.
    why: Option<&'static str>,
    /// The credit it was for.
    credit: String,
    /// When it landed (ms since the epoch).
    at: i64,
    generation: u64,
}

thread_local! {
    /// Agents from the ACP registry's icons, by harness (`acp.<id>`), as the
    /// harness lists have carried them: for the sidebar's rows, whose chats
    /// name only their harness.
    static REGISTRY_ICONS: RefCell<HashMap<String, String>> = RefCell::default();
    /// The same, published to every window's scripts as `agent_icons`
    /// (`{ ["acp.<id>"] = svg }`): ds/agent_mark finds an agent's icon there
    /// by its harness, so no caller has to carry it.
    static ICONS_SHARED: RefCell<Vec<Shared>> = RefCell::default();
}

/// Keeps the icons a harness list carries (agents from the ACP registry).
pub(crate) fn remember_icons(list: &[Json]) {
    remember(list.iter().filter_map(|h| Some((h["id"].as_str()?.to_owned(), h["icon"].as_str()?.to_owned()))));
}

/// Keeps the icons the ACP registry's listing carries (`id`, no `acp.`).
pub(crate) fn remember_listing_icons(list: &[Json]) {
    remember(list.iter().filter_map(|a| Some((format!("acp.{}", a["id"].as_str()?), a["icon"].as_str()?.to_owned()))));
}

fn remember(found: impl Iterator<Item = (String, String)>) {
    let changed = REGISTRY_ICONS.with(|icons| {
        let mut icons = icons.borrow_mut();
        let mut changed = false;
        for (id, icon) in found {
            if id.starts_with("acp.") && !icon.is_empty() && icons.get(&id) != Some(&icon) {
                icons.insert(id, icon);
                changed = true;
            }
        }
        changed
    });
    if changed {
        ICONS_SEEN.with(|n| n.set(n.get() + 1));
        let value = icons_value();
        ICONS_SHARED.with(|shared| {
            for s in shared.borrow().iter() {
                s.set(value.clone());
            }
        });
    }
}

fn icons_value() -> Value {
    let map: serde_json::Map<String, Json> =
        REGISTRY_ICONS.with(|icons| icons.borrow().iter().map(|(k, v)| (k.clone(), json!(v))).collect());
    Value::from_json(&Json::Object(map).to_string()).unwrap_or(Value::Nil)
}

/// The icons asked for once connected: the harnesses here (cheap, cached),
/// then the registry's listing (every agent it has, added anywhere).
async fn learn_icons() {
    if let Ok(Json::Array(list)) = engine::get().call("ListHarnesses", json!({ "includeRegistry": true })).await {
        remember_icons(&list);
    }
    if let Ok(reply) = engine::get().call("ListAcpRegistry", json!({ "refresh": false })).await
        && let Some(list) = reply["agents"].as_array()
    {
        remember_listing_icons(list);
    }
}

thread_local! {
    static ICONS_SEEN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many times the registry's icons have changed: a row built with an
/// icon is built again once this moves.
pub(crate) fn icons_generation() -> u64 {
    ICONS_SEEN.with(std::cell::Cell::get)
}

/// A registry agent's icon, as last listed.
pub(crate) fn registry_icon(harness: &str) -> Option<String> {
    REGISTRY_ICONS.with(|icons| icons.borrow().get(harness).cloned())
}

thread_local! {
    static AGENTS: Rc<Agents> = Rc::new(Agents::new());
}

pub fn install(host: &ScriptHost) {
    let agents = AGENTS.with(Rc::clone);
    agents.shared.borrow_mut().push(host.shared("agents", agents.value()));
    agents.device_shared.borrow_mut().push(host.shared("agents.on_device", agents.device_value()));
    ICONS_SHARED.with(|shared| shared.borrow_mut().push(host.shared("agent_icons", icons_value())));
    if agents.shared.borrow().len() == 1 {
        // Usage moves as turns run: looked at again while the app is open.
        let weak = Rc::downgrade(&agents);
        engine::get().spawn(async move {
            loop {
                zgx::timer::sleep(DEVICE_USAGE_POLL).await;
                let Some(agents) = weak.upgrade() else { return };
                let keys: Vec<String> = agents.device_accounts.borrow().keys().cloned().collect();
                for key in keys {
                    agents.load_device(key, true);
                }
            }
        });
        let weak = Rc::downgrade(&agents);
        engine::get().on_connect(move |rpc| {
            engine::get().spawn(learn_icons());
            let Some(agents) = weak.upgrade() else { return };
            // Versions, as the engine's updater knows them.
            agents.watch_updates(rpc);
            if agents.wanted.get() {
                let agents = agents.clone();
                engine::get().spawn(async move { agents.load().await });
            }
        });
    }

    // Takes the newer version an agent has ready, and how its updates
    // come (notify, auto-when-idle, off).
    let a = agents.clone();
    host.async_action("agents.update", move |id| {
        let params = a.addressed(json!({ "harness": text(&id) }));
        async move {
            engine::get()
                .call("ApplyHarnessUpdate", params)
                .await
                .map(|_| Value::Nil)
                .map_err(|error| engine::outcome(&error).to_string())
        }
    });
    let a = agents.clone();
    host.async_action("agents.set_update_policy", move |args| {
        let id = args.field("id").map(text).unwrap_or_default();
        let policy = args.field("policy").map(text).unwrap_or_default();
        let params = a.addressed(json!({ "harness": id, "policy": policy }));
        async move {
            if !matches!(policy.as_str(), "notify" | "auto-when-idle" | "off") {
                return Err("invalid".to_string());
            }
            engine::get()
                .call("SetHarnessUpdatePolicy", params)
                .await
                .map(|_| Value::Nil)
                .map_err(|error| engine::outcome(&error).to_string())
        }
    });
    // Whose agents to show: a device's id, or nil for this Mac's.
    let a = agents.clone();
    host.action("agents.target", move |id| {
        let id = text(&id);
        a.retarget(if id.is_empty() { None } else { Some(id) });
    });
    let a = agents.clone();
    host.action("agents.watch_device", move |key| a.watch_device(text(&key)));
    let a = agents.clone();
    host.action("agents.refresh_device", move |key| a.load_device(text(&key), true));
    let a = agents.clone();
    host.async_action("agents.switch_on", move |args| {
        let a = a.clone();
        let key = args.field("device").map(text).unwrap_or_default();
        let (id, account) = harness_and_account(&args);
        async move { a.switch_on(key, id, account).await.map_err(String::from) }
    });
    let a = agents.clone();
    host.action("agents.load", move |_| {
        a.wanted.set(true);
        let a = a.clone();
        engine::get().spawn(async move { a.load().await });
    });
    let a = agents.clone();
    host.async_action("agents.sign_in", move |args| {
        let a = a.clone();
        // `id`, or `{ id, method }` (a registry agent's method of its choosing).
        let (id, method) = match args.field("id") {
            Some(id) => (text(&id), args.field("method").map(text).filter(|m| !m.is_empty())),
            None => (text(&args), None),
        };
        async move { crate::plugins::preview::refuse("agents.sign_in")?; a.sign_in(id, method).await.map_err(String::from) }
    });
    let a = agents.clone();
    host.async_action("agents.set_key", move |args| {
        let a = a.clone();
        let id = args.field("id").map(text).unwrap_or_default();
        let var = args.field("var").map(text).unwrap_or_default();
        let value = args.field("value").map(text).unwrap_or_default();
        async move { crate::plugins::preview::refuse("agents.set_key")?; a.set_key(id, var, value).await.map_err(String::from) }
    });
    let a = agents.clone();
    host.action("agents.cancel", move |id| a.cancel(&text(&id)));
    // A registry agent signed out (its own `logout`).
    let a = agents.clone();
    host.async_action("agents.sign_out", move |id| {
        let a = a.clone();
        async move { crate::plugins::preview::refuse("agents.sign_out")?; a.sign_out(text(&id)).await.map_err(String::from) }
    });
    // The page a sign-in under way opened, again (a closed tab, a browser
    // that didn't come forward).
    let a = agents.clone();
    host.action("agents.open_page", move |id| {
        let page = a.logins.borrow().get(&text(&id)).and_then(|l| l.page.clone());
        if let Some(page) = page.filter(|p| openable(p))
            && let Err(error) = crate::auth::open_sign_in(&page)
        {
            eprintln!("zeron-zgx: agents: the browser didn't open: {error}");
        }
    });
    let a = agents.clone();
    host.async_action("agents.complete", move |args| {
        let a = a.clone();
        let id = args.field("id").map(text).unwrap_or_default();
        let code = args.field("code").map(text).unwrap_or_default();
        async move { a.complete(id, code).await.map_err(String::from) }
    });
    let a = agents.clone();
    host.action("agents.dismiss", move |id| {
        if let Some(login) = a.logins.borrow_mut().get_mut(&text(&id)) {
            login.failed = false;
        }
        a.publish();
    });
    let a = agents.clone();
    host.async_action("agents.set_enabled", move |args| {
        let a = a.clone();
        let id = args.field("id").map(text).unwrap_or_default();
        let enabled = matches!(args.field("enabled"), Some(Value::Bool(true)));
        async move { a.set_enabled(id, enabled).await.map_err(String::from) }
    });
    let a = agents.clone();
    host.async_action("agents.switch", move |args| {
        let a = a.clone();
        let (id, account) = harness_and_account(&args);
        async move { a.account_call("ActivateAgentAccount", id, account).await.map_err(String::from) }
    });
    let a = agents.clone();
    host.async_action("agents.remove", move |args| {
        let a = a.clone();
        let (id, account) = harness_and_account(&args);
        async move { crate::plugins::preview::refuse("agents.remove")?; a.account_call("ForgetAgentAccount", id, account).await.map_err(String::from) }
    });
    let a = agents.clone();
    host.async_action("agents.install", move |id| {
        let a = a.clone();
        async move { crate::plugins::preview::refuse("agents.install")?; a.install(text(&id)).await.map_err(String::from) }
    });
    let a = agents.clone();
    host.action("agents.cancel_install", move |id| {
        let id = text(&id);
        a.spawn_call("CancelInstall", json!({ "harness": id }));
    });
    let a = agents.clone();
    host.async_action("agents.use_reset", move |args| {
        let a = a.clone();
        let account = args.field("account").map(text).unwrap_or_default();
        let credit = args.field("credit").map(text).unwrap_or_default();
        async move { a.use_reset(account, credit).await.map_err(String::from) }
    });
    let a = agents;
    host.action("agents.dismiss_reset", move |account| {
        let account = text(&account);
        let mut resets = a.resets.borrow_mut();
        if resets.get(&account).is_some_and(|r| r.state != "using") {
            resets.remove(&account);
            drop(resets);
            a.publish();
        }
    });
}

fn harness_and_account(args: &Value) -> (String, String) {
    (
        args.field("id").map(text).unwrap_or_default(),
        args.field("account").map(text).unwrap_or_default(),
    )
}

/// A sign-in method as the card reads it: `{ id, name, description?, kind,
/// var?, link? }` (`var`: the variable a key method reads).
fn method_value(m: &Json) -> Value {
    let field = |key: &str| m[key].as_str().map_or(Value::Nil, Value::from);
    Value::record([
        ("id", field("id")),
        ("name", field("name")),
        ("description", field("description")),
        ("kind", field("kind")),
        ("var", m["vars"].get(0).and_then(Json::as_str).map_or(Value::Nil, Value::from)),
        ("link", field("link")),
    ])
}

/// The design system's mark for a harness (ds/mark.luau's names).
pub(crate) fn mark(id: &str) -> Option<&'static str> {
    Some(match id {
        "claude-code" => "claude",
        "codex" => "openai",
        "cursor" => "cursor",
        "grok" => "grok",
        "devin" => "devin",
        "opencode" => "opencode",
        "pi" => "pi",
        "hermes" => "hermes",
        "antigravity" => "antigravity",
        "muse" => "muse",
        _ => return None,
    })
}

/// ds.Mark's name for an agent wherever one is named (a chat's row, the
/// switch seam, a model's chip): a built-in's mark, else an agent from the
/// ACP registry's harness itself (`acp.<id>`), which ds.Mark draws as its
/// registry icon, found by that alone.
pub(crate) fn glyph(id: &str) -> Option<String> {
    match mark(id) {
        Some(mark) => Some(mark.to_owned()),
        None if id.starts_with("acp.") => Some(id.to_owned()),
        None => None,
    }
}

/// The sign-in an agent's Add account starts: an agent signing into several
/// providers starts its first (crates/proto/src/providers.rs
/// `login_options`), the one the shipping app lists first.
fn login_provider(id: &str) -> Option<&'static str> {
    match id {
        "opencode" => Some("openai"),
        "pi" | "hermes" => Some("openai-codex"),
        _ => None,
    }
}

/// A page the browser may open for a sign-in: the web, or this Mac's own
/// loopback (crates/ui/src/settings/accounts.rs `is_openable_login_url`).
fn openable(url: &str) -> bool {
    !url.chars().any(char::is_control)
        && (url.starts_with("https://")
            || url.starts_with("http://127.0.0.1:")
            || url.starts_with("http://localhost:"))
}

/// A usage window's fraction, 0 to 1.
fn fraction(window: Option<&Json>) -> f64 {
    window
        .and_then(|w| w["usedFraction"].as_f64())
        .unwrap_or(0.)
        .clamp(0., 1.)
}

impl Agents {
    fn new() -> Self {
        Self {
            shared: RefCell::default(),
            look: Cell::new(Look::Looking),
            harnesses: RefCell::default(),
            accounts: RefCell::default(),
            versions: RefCell::default(),
            update_status: RefCell::default(),
            target: RefCell::default(),
            device_accounts: RefCell::default(),
            device_shared: RefCell::default(),
            installing: RefCell::default(),
            logins: RefCell::default(),
            sign_ins: RefCell::default(),
            updates: RefCell::default(),
            wanted: Cell::new(false),
            loading: Cell::new(0),
            resets: RefCell::default(),
            reset_keys: RefCell::default(),
            reset_generation: Cell::new(0),
        }
    }

    fn value(&self) -> Value {
        let state = match self.look.get() {
            Look::Looking => "looking",
            Look::Ready => "ready",
            Look::Unavailable => "unavailable",
        };
        let versions = self.versions.borrow();
        let updates = self.update_status.borrow();
        let installing = self.installing.borrow();
        let accounts = self.accounts.borrow();
        let logins = self.logins.borrow();
        let sign_ins = self.sign_ins.borrow();
        let resets = self.resets.borrow();
        let account_value = |account: &Json| {
            let reset = account["id"].as_str().and_then(|id| resets.get(id));
            account_value(account, reset)
        };
        // Each agent whole, its accounts and sign-in with it: a list a page
        // keys by `id` and updates row by row.
        let list: Vec<Value> = self
            .harnesses
            .borrow()
            .iter()
            .filter(|h| h["id"].as_str().is_some_and(|id| id != "mock"))
            .map(|h| {
                let id = h["id"].as_str().unwrap_or_default();
                let rows: Vec<Value> = accounts
                    .get(id)
                    .map(|list| list.iter().map(account_value).collect())
                    .unwrap_or_default();
                let login = match logins.get(id) {
                    Some(login) if login.waiting && login.code => "code",
                    Some(login) if login.waiting => "waiting",
                    Some(login) if login.failed => "failed",
                    _ => "idle",
                };
                Value::record([
                    ("id", Value::from(id)),
                    ("name", Value::from(h["name"].as_str().unwrap_or(id))),
                    ("mark", mark(id).map_or(Value::Nil, Value::from)),
                    // An agent added from the ACP registry: its one-ink icon
                    // (it has no mark), and no accounts of Zeron's to show.
                    ("registry", Value::Bool(id.starts_with("acp."))),
                    ("icon", h["icon"].as_str().map_or(Value::Nil, Value::from)),
                    ("installed", Value::Bool(h["installed"].as_bool().unwrap_or(true))),
                    ("can_install", Value::Bool(h["canInstall"].as_bool().unwrap_or(false))),
                    // Unknown (an older engine): on, as the shipping app reads it.
                    ("enabled", Value::Bool(h["enabled"].as_bool().unwrap_or(true))),
                    ("version", versions.get(id).map_or(Value::Nil, |v| Value::from(v.as_str()))),
                    (
                        "latest",
                        updates.get(id).and_then(|u| u.latest.as_deref()).map_or(Value::Nil, Value::from),
                    ),
                    (
                        "update_policy",
                        Value::from(updates.get(id).map_or("notify", |u| u.policy.as_str())),
                    ),
                    ("updating", Value::Bool(updates.get(id).is_some_and(|u| u.updating))),
                    ("installing", Value::Bool(installing.iter().any(|i| i == id))),
                    ("login", Value::from(login)),
                    (
                        "login_page",
                        logins
                            .get(id)
                            .and_then(|l| l.page.as_deref())
                            .filter(|_| login == "waiting")
                            .map_or(Value::Nil, Value::from),
                    ),
                    (
                        "login_command",
                        logins
                            .get(id)
                            .and_then(|l| l.command.as_deref())
                            .filter(|_| login == "failed")
                            .map_or(Value::Nil, Value::from),
                    ),
                    (
                        "login_note",
                        logins
                            .get(id)
                            .and_then(|l| l.note.as_deref())
                            .filter(|_| login != "idle")
                            .map_or(Value::Nil, Value::from),
                    ),
                    (
                        "login_code",
                        logins
                            .get(id)
                            .and_then(|l| l.pairing.as_deref())
                            .filter(|_| login == "waiting")
                            .map_or(Value::Nil, Value::from),
                    ),
                    // A registry agent's sign-in, once it's said (nil until).
                    (
                        "signed_in",
                        sign_ins.get(id).map_or(Value::Nil, |s| Value::Bool(s.signed_in)),
                    ),
                    (
                        "signed_in_as",
                        sign_ins.get(id).and_then(|s| s.account.as_deref()).map_or(Value::Nil, Value::from),
                    ),
                    ("can_sign_out", Value::Bool(sign_ins.get(id).is_some_and(|s| s.can_sign_out))),
                    ("signing_out", Value::Bool(sign_ins.get(id).is_some_and(|s| s.signing_out))),
                    (
                        "login_methods",
                        Value::List(
                            sign_ins
                                .get(id)
                                .map(|s| s.methods.iter().map(method_value).collect::<Vec<_>>())
                                .unwrap_or_default()
                                .into(),
                        ),
                    ),
                    ("accounts", Value::List(rows.into())),
                ])
            })
            .collect();
        let target = self.target.borrow().as_deref().map_or(Value::Nil, Value::from);
        Value::record([
            ("state", Value::from(state)),
            ("list", Value::List(list.into())),
            ("target", target),
        ])
    }

    fn publish(&self) {
        let value = self.value();
        for shared in self.shared.borrow().iter() {
            shared.set(value.clone());
        }
    }

    /// `params`, addressed to the device shown.
    fn addressed(&self, params: Json) -> Json {
        addressed_to(self.target.borrow().as_deref(), params)
    }

    /// A call to `target` (none: this Mac's engine) nobody waits on.
    fn spawn_call_to(&self, target: Option<&str>, method: &'static str, params: Json) {
        let params = addressed_to(target, params);
        engine::get().spawn(async move {
            if let Err(error) = engine::get().call(method, params).await {
                eprintln!("zeron-zgx: agents: {method} failed: {error}");
            }
        });
    }

    /// The updates watch, on `rpc`, for the device shown.
    fn watch_updates(self: &Rc<Self>, rpc: &crate::engine::rpc::Rpc) {
        let heard = Rc::downgrade(self);
        let watch = rpc.watch("WatchHarnessUpdates", self.addressed(json!({})), move |item| {
            if let Some(agents) = heard.upgrade() {
                agents.heard_versions(&item);
            }
        });
        *self.updates.borrow_mut() = Some(watch);
    }

    /// Shows another device's agents (or this Mac's, `None`): what was shown
    /// goes, and the new device's are looked for.
    fn retarget(self: &Rc<Self>, target: Option<String>) {
        if *self.target.borrow() == target {
            return;
        }
        *self.target.borrow_mut() = target;
        // A sign-in under way on the device left is given up there (its
        // poll stops as its entry goes).
        let leaving: Vec<(Option<String>, String)> = self
            .logins
            .borrow()
            .values()
            .filter(|login| login.waiting)
            .filter_map(|login| Some((login.target.clone(), login.login_id.clone()?)))
            .collect();
        for (device, login_id) in leaving {
            self.spawn_call_to(device.as_deref(), "CancelAgentLogin", json!({ "loginId": login_id }));
        }
        self.harnesses.borrow_mut().clear();
        self.accounts.borrow_mut().clear();
        self.versions.borrow_mut().clear();
        self.update_status.borrow_mut().clear();
        self.installing.borrow_mut().clear();
        self.logins.borrow_mut().clear();
        self.sign_ins.borrow_mut().clear();
        // (A reset under way finishes on its device; its word isn't this
        // one's. Keys stay: the same credit is the same request anywhere.)
        self.resets.borrow_mut().clear();
        self.reset_generation.set(self.reset_generation.get() + 1);
        self.updates.borrow_mut().take();
        self.look.set(Look::Looking);
        self.publish();
        if let Some(rpc) = engine::get().current() {
            self.watch_updates(&rpc);
        }
        let agents = self.clone();
        engine::get().spawn(async move { agents.load().await });
    }

    fn spawn_call(&self, method: &'static str, params: Json) {
        let params = self.addressed(params);
        engine::get().spawn(async move {
            if let Err(error) = engine::get().call(method, params).await {
                eprintln!("zeron-zgx: agents: {method} failed: {error}");
            }
        });
    }

    /// The registry, then the accounts (as last known, then with fresh
    /// usage), as the shipping app paints them. A painted list stays while
    /// it looks again.
    async fn load(self: Rc<Self>) {
        let run = self.loading.get() + 1;
        self.loading.set(run);
        if self.look.get() != Look::Ready {
            self.look.set(Look::Looking);
            self.publish();
        }
        match engine::get()
            .call("ListHarnesses", self.addressed(json!({ "includeRegistry": true })))
            .await
        {
            Ok(Json::Array(list)) if self.loading.get() == run => {
                remember_icons(&list);
                *self.harnesses.borrow_mut() = list;
                self.look.set(Look::Ready);
                self.publish();
            }
            Ok(_) if self.loading.get() != run => return,
            Ok(_) | Err(_) => {
                if self.loading.get() == run && self.look.get() != Look::Ready {
                    self.look.set(Look::Unavailable);
                    self.publish();
                }
                return;
            }
        }
        self.load_accounts(false).await;
        self.load_accounts(true).await;
        // Each registry agent's sign-in, asked of it (it's started for
        // that, so after the rest, and only those switched on).
        let registry: Vec<String> = self
            .harnesses
            .borrow()
            .iter()
            .filter(|h| h["enabled"].as_bool().unwrap_or(true) && h["installed"].as_bool().unwrap_or(true))
            .filter_map(|h| h["id"].as_str().filter(|id| id.starts_with("acp.")).map(str::to_owned))
            .collect();
        for id in registry {
            let agents = self.clone();
            engine::get().spawn(async move { agents.load_sign_in(id, false).await });
        }
    }

    /// A registry agent's sign-in, as it says it is (`refresh`: asked again,
    /// not as it last said).
    async fn load_sign_in(self: Rc<Self>, id: String, refresh: bool) {
        let Some(registry_id) = id.strip_prefix("acp.") else { return };
        let asked_of = self.target.borrow().clone();
        let said = engine::get()
            .call("GetAcpAgentSignIn", self.addressed(json!({ "id": registry_id, "refresh": refresh })))
            .await;
        if *self.target.borrow() != asked_of {
            return;
        }
        match said {
            Ok(state) => {
                let signing_out = self.sign_ins.borrow().get(&id).is_some_and(|s| s.signing_out);
                self.sign_ins.borrow_mut().insert(
                    id,
                    SignIn {
                        signed_in: state["signedIn"].as_bool().unwrap_or(false),
                        account: state["account"].as_str().map(str::to_owned),
                        can_sign_out: state["canSignOut"].as_bool().unwrap_or(false),
                        signing_out,
                        methods: state["methods"].as_array().cloned().unwrap_or_default(),
                    },
                );
                self.publish();
            }
            // (Not told: the row offers the sign-in, as before.)
            Err(error) => eprintln!("zeron-zgx: agents: {id} didn't say whether it's signed in: {error}"),
        }
    }

    /// A key a registry agent reads from its environment (`var`), kept in
    /// its environment on the device shown; then it's asked again whether
    /// it's signed in.
    async fn set_key(self: Rc<Self>, id: String, var: String, value: String) -> Result<Value, Outcome> {
        let Some(registry_id) = id.strip_prefix("acp.").map(str::to_owned) else { return Err("failed") };
        let params = json!({ "id": registry_id, "var": var, "value": value.trim() });
        match engine::get().call("SetAcpAgentKey", self.addressed(params)).await {
            Ok(_) => {
                self.clone().load_sign_in(id, true).await;
                Ok(Value::Nil)
            }
            Err(error) => {
                eprintln!("zeron-zgx: agents: {id}'s key wasn't kept: {error}");
                Err(fail(&error))
            }
        }
    }

    /// Signs a registry agent out (its own `logout`), then asks again.
    async fn sign_out(self: Rc<Self>, id: String) -> Result<Value, Outcome> {
        let Some(registry_id) = id.strip_prefix("acp.").map(str::to_owned) else { return Err("failed") };
        if let Some(state) = self.sign_ins.borrow_mut().get_mut(&id) {
            state.signing_out = true;
        }
        self.publish();
        let outcome = engine::get().call("SignOutAcpAgent", self.addressed(json!({ "id": registry_id }))).await;
        if let Some(state) = self.sign_ins.borrow_mut().get_mut(&id) {
            state.signing_out = false;
        }
        self.publish();
        match outcome {
            Ok(_) => {
                self.clone().load_sign_in(id, true).await;
                Ok(Value::Nil)
            }
            Err(error) => {
                eprintln!("zeron-zgx: agents: {id} didn't sign out: {error}");
                Err(fail(&error))
            }
        }
    }

    async fn load_accounts(&self, fresh_usage: bool) {
        let params = self.addressed(json!({ "forceUsage": fresh_usage }));
        let asked_of = self.target.borrow().clone();
        match engine::get().call("ListAgentAccounts", params).await {
            // (Another device's, asked before the switch: not shown.)
            Ok(_) if *self.target.borrow() != asked_of => {}
            Ok(snapshot) => self.take_accounts(&snapshot),
            Err(error) => eprintln!("zeron-zgx: agents: accounts unavailable: {error}"),
        }
    }

    /// An `AgentAccountsSnapshot`: `{accounts, warnings}`.
    fn take_accounts(&self, snapshot: &Json) {
        let mut by: BTreeMap<String, Vec<Json>> = BTreeMap::new();
        for account in snapshot["accounts"].as_array().into_iter().flatten() {
            if let Some(harness) = account["harness"].as_str() {
                by.entry(harness.into()).or_default().push(account.clone());
            }
        }
        *self.accounts.borrow_mut() = by;
        self.publish();
        // (The composers read the same accounts: a switch in Settings shows
        // there too.)
        let key = self.target.borrow().clone().unwrap_or_default();
        self.keep_device(key, snapshot);
    }

    fn device_value(&self) -> Value {
        let devices = self.device_accounts.borrow();
        Value::record(devices.iter().map(|(key, device)| (key.as_str(), device_accounts_value(&device.accounts))))
    }

    fn publish_devices(&self) {
        let value = self.device_value();
        for shared in self.device_shared.borrow().iter() {
            shared.set(value.clone());
        }
    }

    fn keep_device(&self, key: String, snapshot: &Json) {
        let Some(accounts) = snapshot["accounts"].as_array() else { return };
        self.device_accounts.borrow_mut().entry(key).or_default().accounts = accounts.clone();
        self.publish_devices();
    }

    /// A composer on `key`'s device: its accounts, on first sight of it.
    fn watch_device(self: &Rc<Self>, key: String) {
        let first = {
            let mut devices = self.device_accounts.borrow_mut();
            let device = devices.entry(key.clone()).or_default();
            !std::mem::replace(&mut device.asked, true)
        };
        if first {
            self.load_device(key, true);
        }
    }

    /// `key`'s accounts again. Forced, the engine asks the provider, so not
    /// more than `DEVICE_USAGE_MIN` apart; with nothing yet, what the engine
    /// kept paints first.
    fn load_device(self: &Rc<Self>, key: String, force: bool) {
        let paint_first = {
            let mut devices = self.device_accounts.borrow_mut();
            let device = devices.entry(key.clone()).or_default();
            if force {
                if device.last_forced.is_some_and(|at| at.elapsed() < DEVICE_USAGE_MIN) {
                    return;
                }
                device.last_forced = Some(Instant::now());
            }
            force && device.accounts.is_empty()
        };
        let me = self.clone();
        engine::get().spawn(async move {
            let target = (!key.is_empty()).then_some(key.as_str());
            let params = |force: bool| addressed_to(target, json!({ "forceUsage": force }));
            if paint_first && let Ok(snapshot) = engine::get().call("ListAgentAccounts", params(false)).await {
                me.keep_device(key.clone(), &snapshot);
            }
            if let Ok(snapshot) = engine::get().call("ListAgentAccounts", params(force)).await {
                me.keep_device(key.clone(), &snapshot);
            }
        });
    }

    /// That account in use on `key`'s device: its row at once, its login's
    /// group (its agent, or its model provider where an agent keeps one per
    /// provider); the engine's reply replaces them, a refusal restores them.
    async fn switch_on(self: Rc<Self>, key: String, id: String, account: String) -> Result<Value, &'static str> {
        let before = self.device_accounts.borrow().get(&key).map(|d| d.accounts.clone());
        if let Some(device) = self.device_accounts.borrow_mut().get_mut(&key) {
            mark_switched(&mut device.accounts, &id, &account);
        }
        self.publish_devices();
        let target = (!key.is_empty()).then_some(key.as_str());
        let params = addressed_to(target, json!({ "harness": id, "accountId": account }));
        match engine::get().call("ActivateAgentAccount", params).await {
            Ok(snapshot) => {
                self.keep_device(key.clone(), &snapshot);
                if self.target.borrow().as_deref().unwrap_or_default() == key {
                    self.take_accounts(&snapshot);
                }
                Ok(Value::Nil)
            }
            Err(error) => {
                eprintln!("zeron-zgx: agents: ActivateAgentAccount failed: {error}");
                if let (Some(before), Some(device)) = (before, self.device_accounts.borrow_mut().get_mut(&key)) {
                    device.accounts = before;
                }
                self.publish_devices();
                Err(fail(&error))
            }
        }
    }

    fn heard_versions(&self, item: &Json) {
        let mut versions = HashMap::new();
        let mut updates = HashMap::new();
        for status in item.as_array().into_iter().flatten() {
            if let (Some(id), Some(version)) = (status["harness"].as_str(), status["installedVersion"].as_str()) {
                versions.insert(id.to_string(), version.to_string());
            }
            if let Some(id) = status["harness"].as_str() {
                updates.insert(id.to_string(), Update::from(status));
            }
        }
        if *self.versions.borrow() != versions || *self.update_status.borrow() != updates {
            *self.versions.borrow_mut() = versions;
            *self.update_status.borrow_mut() = updates;
            self.publish();
        }
    }

    fn login_generation(&self, id: &str) -> u64 {
        self.logins.borrow().get(id).map_or(0, |l| l.generation)
    }

    fn settle(&self, id: &str, generation: u64, failed: bool) {
        if let Some(login) = self.logins.borrow_mut().get_mut(id)
            && login.generation == generation
        {
            login.waiting = false;
            login.failed = failed;
            login.code = false;
        }
        self.publish();
    }

    /// A sign-in that asked for a code, finished with the one pasted back.
    async fn complete(self: Rc<Self>, id: String, code: String) -> Result<Value, Outcome> {
        let (generation, login_id, device) = match self.logins.borrow().get(&id) {
            Some(login) if login.waiting && login.code => {
                (login.generation, login.login_id.clone().unwrap_or_default(), login.target.clone())
            }
            _ => return Err("cancelled"),
        };
        let params = addressed_to(device.as_deref(), json!({ "loginId": login_id, "code": code.trim() }));
        match engine::get().call("CompleteAgentLogin", params).await {
            Ok(snapshot) => {
                if self.login_generation(&id) != generation {
                    return Err("cancelled");
                }
                self.take_accounts(&snapshot);
                self.settle(&id, generation, false);
                self.load_accounts(true).await;
                Ok(Value::Nil)
            }
            Err(error) => {
                eprintln!("zeron-zgx: agents: {id}'s code didn't finish the sign-in: {error}");
                self.settle(&id, generation, true);
                Err(fail(&error))
            }
        }
    }

    /// A new account: the engine's sign-in (`StartAgentLogin`), its page
    /// opened, then asked after (`PollAgentLogin`) until it lands or fails.
    async fn sign_in(self: Rc<Self>, id: String, method: Option<String>) -> Result<Value, Outcome> {
        // The device it's on, for all of it (the page may show another).
        let device = self.target.borrow().clone();
        let device = device.as_deref();
        let generation = {
            let mut logins = self.logins.borrow_mut();
            let login = logins.entry(id.clone()).or_default();
            login.generation += 1;
            login.waiting = true;
            login.failed = false;
            login.code = false;
            login.note = None;
            login.command = None;
            login.page = None;
            login.pairing = None;
            login.target = device.map(Into::into);
            login.generation
        };
        self.publish();
        let mut params = json!({ "harness": id });
        if let Some(provider) = login_provider(&id) {
            params["provider"] = json!(provider);
        }
        // (A registry agent's method of its choosing, in the same field.)
        if let Some(method) = method.filter(|_| id.starts_with("acp.")) {
            params["provider"] = json!(method);
        }
        let start = match engine::get().call("StartAgentLogin", addressed_to(device, params)).await {
            Ok(start) => start,
            Err(error) => {
                eprintln!("zeron-zgx: agents: {id}'s sign-in didn't start: {error}");
                self.settle(&id, generation, true);
                return Err(fail(&error));
            }
        };
        let login_id = start["loginId"].as_str().unwrap_or_default().to_string();
        match self.logins.borrow_mut().get_mut(&id) {
            Some(login) if login.generation == generation => login.login_id = Some(login_id.clone()),
            // Cancelled while it started: the engine's side goes too.
            _ => {
                self.spawn_call_to(device, "CancelAgentLogin", json!({ "loginId": login_id }));
                return Err("cancelled");
            }
        }
        if start["mode"].as_str() == Some("paste-code") {
            // The page shows a code once you've signed in, pasted back
            // (`agents.complete`): the card asks for it.
            if let Some(url) = start["url"].as_str().filter(|u| !u.is_empty() && openable(u))
                && let Err(error) = crate::auth::open_sign_in(url)
            {
                eprintln!("zeron-zgx: agents: the browser didn't open: {error}");
            }
            if let Some(login) = self.logins.borrow_mut().get_mut(&id) {
                login.code = true;
            }
            self.publish();
            return Ok(Value::Nil);
        }
        let mut opened = None::<String>;
        let this = self.clone();
        let open_id = id.clone();
        let mut open = |url: Option<&str>| {
            if let Some(url) = url.filter(|u| !u.is_empty() && openable(u))
                && opened.as_deref() != Some(url)
            {
                if let Err(error) = crate::auth::open_sign_in(url) {
                    eprintln!("zeron-zgx: agents: the browser didn't open: {error}");
                }
                opened = Some(url.to_string());
                // Kept, for the card to open again.
                if let Some(login) = this.logins.borrow_mut().get_mut(&open_id)
                    && login.generation == generation
                {
                    login.page = Some(url.to_string());
                }
                this.publish();
            }
        };
        open(start["url"].as_str());
        let until = Instant::now() + SIGN_IN_WAIT;
        loop {
            zgx::timer::sleep(POLL).await;
            if self.login_generation(&id) != generation {
                return Err("cancelled");
            }
            if Instant::now() >= until {
                self.spawn_call_to(device, "CancelAgentLogin", json!({ "loginId": login_id }));
                self.settle(&id, generation, true);
                return Err("failed");
            }
            let poll = match engine::get().call("PollAgentLogin", addressed_to(device, json!({ "loginId": login_id }))).await {
                Ok(poll) => poll,
                Err(error) => {
                    eprintln!("zeron-zgx: agents: lost {id}'s sign-in: {error}");
                    self.settle(&id, generation, true);
                    return Err(fail(&error));
                }
            };
            if self.login_generation(&id) != generation {
                return Err("cancelled");
            }
            match poll["status"].as_str() {
                Some("done") => {
                    self.load_accounts(false).await;
                    if id.starts_with("acp.") {
                        // (Before it settles: the row goes from the sign-in
                        // to signed in, not back to the sign-in first.)
                        self.clone().load_sign_in(id.clone(), true).await;
                    }
                    self.settle(&id, generation, false);
                    self.load_accounts(true).await;
                    return Ok(Value::Nil);
                }
                Some("error") => {
                    eprintln!(
                        "zeron-zgx: agents: {id}'s sign-in failed: {}",
                        poll["message"].as_str().unwrap_or("no reason given")
                    );
                    // An agent from the ACP registry says why in its own
                    // words (one that needs a terminal names the command).
                    if id.starts_with("acp.") {
                        self.note(&id, generation, poll["message"].as_str());
                    }
                    self.settle(&id, generation, true);
                    return Err("failed");
                }
                // Pending: a page learned late opens once, and a device
                // code to type in is said.
                _ => {
                    open(poll["url"].as_str());
                    let code = poll["code"].as_str().map(str::to_owned);
                    let changed = match self.logins.borrow_mut().get_mut(&id) {
                        Some(login) if login.generation == generation && login.pairing != code => {
                            login.pairing = code;
                            true
                        }
                        _ => false,
                    };
                    if changed {
                        self.publish();
                    }
                    self.note(&id, generation, poll["message"].as_str());
                }
            }
        }
    }

    /// The sign-in's note (a device code, a reason), if it changed. A
    /// sign-in that needs a terminal names its command after
    /// [`TERMINAL_COMMAND`]: kept apart, for the terminal to run.
    fn note(&self, id: &str, generation: u64, note: Option<&str>) {
        let (note, command) = match note.and_then(|n| n.split_once(TERMINAL_COMMAND)) {
            Some((said, command)) => (Some(said.trim()), Some(command.trim())),
            None => (note, None),
        };
        let changed = match self.logins.borrow_mut().get_mut(id) {
            Some(login)
                if login.generation == generation
                    && (login.note.as_deref() != note || login.command.as_deref() != command) =>
            {
                login.note = note.map(Into::into);
                login.command = command.map(Into::into);
                true
            }
            _ => false,
        };
        if changed {
            self.publish();
        }
    }

    fn cancel(&self, id: &str) {
        let mut logins = self.logins.borrow_mut();
        let Some(login) = logins.get_mut(id) else { return };
        login.generation += 1;
        let was_waiting = login.waiting;
        login.waiting = false;
        login.failed = false;
        login.code = false;
        let login_id = login.login_id.take();
        let device = login.target.clone();
        drop(logins);
        self.publish();
        // The engine's side of it, on the device it's on (it reaps an
        // abandoned one after 15 minutes, too).
        if let (true, Some(login_id)) = (was_waiting, login_id) {
            self.spawn_call_to(device.as_deref(), "CancelAgentLogin", json!({ "loginId": login_id }));
        }
    }

    async fn set_enabled(&self, id: String, enabled: bool) -> Result<Value, Outcome> {
        match engine::get()
            .call(
                "SetHarnessEnabled",
                self.addressed(json!({ "harness": id, "enabled": enabled, "includeRegistry": true })),
            )
            .await
        {
            Ok(Json::Array(list)) => {
                remember_icons(&list);
                *self.harnesses.borrow_mut() = list;
                self.publish();
                Ok(Value::Nil)
            }
            Ok(_) => Ok(Value::Nil),
            Err(error) => {
                eprintln!("zeron-zgx: agents: {id} couldn't be turned {}: {error}", if enabled { "on" } else { "off" });
                // The list as the engine has it, so the switch goes back.
                self.publish();
                Err(fail(&error))
            }
        }
    }

    async fn account_call(&self, method: &'static str, id: String, account: String) -> Result<Value, Outcome> {
        match engine::get()
            .call(method, self.addressed(json!({ "harness": id, "accountId": account })))
            .await
        {
            Ok(snapshot) => {
                self.take_accounts(&snapshot);
                Ok(Value::Nil)
            }
            Err(error) => {
                eprintln!("zeron-zgx: agents: {method} failed: {error}");
                Err(fail(&error))
            }
        }
    }

    /// Spends one of a Codex account's banked resets, on the device shown
    /// (the account's: the list is that device's, and the engine forwards
    /// there). The engine checks the saved login against its token first;
    /// whatever Codex says lands on the account's row.
    async fn use_reset(self: Rc<Self>, account: String, credit: String) -> Result<Value, Outcome> {
        if account.is_empty() || credit.is_empty() {
            return Err("failed");
        }
        if self.resets.borrow().get(&account).is_some_and(|r| r.state == "using") {
            return Err("busy");
        }
        let device = self.target.borrow().clone();
        let key = self
            .reset_keys
            .borrow_mut()
            .entry((account.clone(), credit.clone()))
            .or_insert_with(idempotency_key)
            .clone();
        let generation = self.reset_generation.get() + 1;
        self.reset_generation.set(generation);
        self.resets.borrow_mut().insert(
            account.clone(),
            Reset { state: "using", why: None, credit: credit.clone(), at: now_ms(), generation },
        );
        self.publish();
        let params = addressed_to(
            device.as_deref(),
            json!({ "accountId": account, "creditId": credit, "idempotencyKey": key }),
        );
        let reply = engine::get().call("ConsumeCodexResetCredit", params).await;
        // Another device shown since, or its word dismissed: not this row's.
        if *self.target.borrow() != device
            || self.resets.borrow().get(&account).is_none_or(|r| r.generation != generation)
        {
            return Err("cancelled");
        }
        let (state, why) = match &reply {
            Ok(outcome) => reset_state(outcome["outcome"].as_str()),
            Err(error) => {
                eprintln!("zeron-zgx: agents: the reset wasn't used: {error}");
                ("failed", Some(reset_failure(error)))
            }
        };
        // A definite answer spends the key; a failure keeps it, so trying
        // again is the same request.
        if state != "failed" {
            self.reset_keys.borrow_mut().remove(&(account.clone(), credit.clone()));
        }
        self.resets
            .borrow_mut()
            .insert(account.clone(), Reset { state, why, credit, at: now_ms(), generation });
        self.publish();
        if state == "reset" {
            // The limits started over: said for a while, then the row is as
            // the usage says.
            let weak = Rc::downgrade(&self);
            let account = account.clone();
            engine::get().spawn(async move {
                zgx::timer::sleep(RESET_SHOWN).await;
                let Some(agents) = weak.upgrade() else { return };
                let mut resets = agents.resets.borrow_mut();
                if resets.get(&account).is_some_and(|r| r.generation == generation) {
                    resets.remove(&account);
                    drop(resets);
                    agents.publish();
                }
            });
        }
        if state != "failed" {
            // Fresh usage and credits (the engine lets this probe through
            // at once after a reset).
            self.load_accounts(true).await;
        }
        match why {
            Some(why) => Err(why),
            None => Ok(Value::from(state)),
        }
    }

    async fn install(&self, id: String) -> Result<Value, Outcome> {
        if self.installing.borrow().contains(&id) {
            return Err("failed");
        }
        self.installing.borrow_mut().push(id.clone());
        self.publish();
        let result = engine::get().call("InstallHarness", self.addressed(json!({ "harness": id }))).await;
        self.installing.borrow_mut().retain(|i| *i != id);
        match result {
            Ok(Json::Array(list)) => {
                remember_icons(&list);
                *self.harnesses.borrow_mut() = list;
                self.publish();
                Ok(Value::Nil)
            }
            Ok(_) => {
                self.publish();
                Ok(Value::Nil)
            }
            Err(error) => {
                eprintln!("zeron-zgx: agents: {id} didn't install: {error}");
                self.publish();
                Err(fail(&error))
            }
        }
    }
}

/// An engine error as an outcome: offline, or failed.
fn fail(error: &str) -> Outcome {
    match engine::outcome(error) {
        "offline" => "offline",
        _ => "failed",
    }
}

/// What Codex said of a reset (`CodexResetOutcome.outcome`), as the row's
/// word: the limits started over (or had already: the same reset asked
/// twice), no window needed it (still banked), or it's no longer there.
fn reset_state(outcome: Option<&str>) -> (&'static str, Option<&'static str>) {
    match outcome {
        Some("reset" | "already_redeemed") => ("reset", None),
        Some("nothing_to_reset") => ("nothing", None),
        Some("no_credit") => ("gone", None),
        // (Unknown: maybe spent. The key stays, so another go is the same
        // request.)
        _ => ("failed", Some("failed")),
    }
}

/// Why the engine refused a reset (crates/engine agent_accounts.rs
/// `consume_codex_reset_credit`'s words), as an outcome: Codex answered for
/// another account than the one saved, the saved login can't be checked,
/// the account is no longer saved, or the device can't be reached.
fn reset_failure(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if engine::outcome(error) == "offline" {
        "offline"
    } else if lower.contains("different account") {
        "mismatch"
    } else if lower.contains("could not be verified") || lower.contains("needs to be refreshed") {
        "signed_out"
    } else if lower.contains("no longer saved") {
        "missing"
    } else {
        "failed"
    }
}

/// A fresh idempotency key (a v4 UUID's shape, from std's per-process
/// random hasher keys and the clock).
fn idempotency_key() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut bytes = [0u8; 16];
    for half in bytes.chunks_mut(8) {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(nanos);
        half.copy_from_slice(&hasher.finish().to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A Codex account's banked resets (`codexResetCredits`), soonest to expire
/// first, less the one a reset just spent (`spent`) until fresh credits
/// come: its count and the credits.
fn banked(summary: &Json, spent: Option<&str>) -> (u64, Vec<Value>) {
    let mut credits: Vec<&Json> = summary["credits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["id"].as_str().is_some_and(|id| Some(id) != spent))
        .collect();
    let expires = |c: &Json| c["expiresAt"].as_str().and_then(epoch_secs);
    // (Stable: Codex's order among the undated, which go last.)
    credits.sort_by_key(|c| expires(c).unwrap_or(i64::MAX));
    let mut count = summary["availableCount"].as_u64().unwrap_or(credits.len() as u64);
    if spent.is_some() {
        count = count.saturating_sub(1);
    }
    let list = credits
        .into_iter()
        .map(|c| {
            let mut fields = vec![
                ("id", Value::from(c["id"].as_str().unwrap_or_default())),
                ("title", Value::from(c["title"].as_str().unwrap_or("Full reset"))),
            ];
            if let Some(scope) = c["description"].as_str().filter(|s| !s.is_empty()) {
                fields.push(("scope", Value::from(scope)));
            }
            if let Some(at) = expires(c) {
                fields.push(("expires", Value::from(at as f64)));
            }
            Value::record(fields)
        })
        .collect();
    (count, list)
}

/// An account as the card takes it: who, on what plan, its two windows, in
/// use or not; a Codex account's banked resets, and how its last went.
fn account_value(account: &Json, reset: Option<&Reset>) -> Value {
    let windows = account["usageWindows"].as_array().cloned().unwrap_or_default();
    // The session is the first window; the week the one that says so, or the
    // second.
    let week = windows
        .iter()
        .position(|w| {
            w["label"]
                .as_str()
                .is_some_and(|l| l.to_ascii_lowercase().contains("week") || l.contains('7'))
        })
        .or((windows.len() > 1).then_some(1));
    let who = account_name(account);
    // A reset that landed since the last usage probe: what it did shows (the
    // windows at 0, that reset gone) until the next probe says.
    let stale = |r: &Reset| account["usageFetchedAt"].as_i64().is_none_or(|at| at <= r.at);
    let started_over = reset.is_some_and(|r| r.state == "reset" && stale(r));
    let spent = reset.filter(|r| matches!(r.state, "reset" | "gone") && stale(r)).map(|r| r.credit.as_str());
    let (session, week_used) = if started_over {
        (0., 0.)
    } else {
        (fraction(windows.first()), fraction(week.and_then(|i| windows.get(i))))
    };
    let mut fields = vec![
        ("id", Value::from(account["id"].as_str().unwrap_or_default())),
        ("email", Value::from(who)),
        ("plan", Value::from(account["planLabel"].as_str().unwrap_or(""))),
        ("session", Value::from(session)),
        ("week", Value::from(week_used)),
        ("active", Value::Bool(account["active"].as_bool().unwrap_or(false))),
    ];
    if account["codexResetCredits"].is_object() {
        let (count, list) = banked(&account["codexResetCredits"], spent);
        fields.push(("banked", Value::from(count as f64)));
        fields.push(("resets", Value::List(list.into())));
    }
    if let Some(r) = reset {
        let mut word = vec![("state", Value::from(r.state))];
        if let Some(why) = r.why {
            word.push(("why", Value::from(why)));
        }
        fields.push(("reset", Value::record(word)));
    }
    // When each window starts over (seconds since the epoch), for the card
    // to say in this Mac's time.
    for (name, window) in [("session_resets", windows.first()), ("week_resets", week.and_then(|i| windows.get(i)))] {
        if let Some(at) = window.and_then(|w| w["resetsAt"].as_str()).and_then(epoch_secs) {
            fields.push((name, Value::from(at as f64)));
        }
    }
    Value::record(fields)
}

/// One device's accounts, for its composers.
#[derive(Default)]
struct DeviceAccounts {
    accounts: Vec<Json>,
    /// A composer has asked for them.
    asked: bool,
    last_forced: Option<Instant>,
}

/// Forced usage asks the provider: not more often than this.
const DEVICE_USAGE_MIN: Duration = Duration::from_secs(30);
/// How often usage is looked at again while the app is open.
const DEVICE_USAGE_POLL: Duration = Duration::from_secs(5 * 60);

/// `account` the one in use of its group: its agent, or for an agent that
/// keeps a login per model provider, that provider; other groups keep theirs.
fn mark_switched(accounts: &mut [Json], harness: &str, account: &str) {
    let provider = accounts
        .iter()
        .find(|a| a["harness"] == harness && a["id"] == account)
        .map(|a| a["provider"].clone());
    let Some(provider) = provider else { return };
    for row in accounts.iter_mut() {
        if row["harness"] == harness && row["provider"] == provider {
            row["active"] = Json::Bool(row["id"] == account);
        }
    }
}

/// A device's accounts as the composer reads them, by agent: only an agent
/// that signs in and reports its usage. `{ id, email, plan, active,
/// can_switch, error?, windows = { { label, used } } }`.
fn device_accounts_value(accounts: &[Json]) -> Value {
    let mut by: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for account in accounts {
        let Some(harness) = account["harness"].as_str() else { continue };
        let Ok(id) = serde_json::from_value::<zeron_proto::HarnessId>(account["harness"].clone()) else {
            continue;
        };
        if !(zeron_proto::providers::signs_in(id) && zeron_proto::providers::reports_usage(id)) {
            continue;
        }
        let windows: Vec<Value> = account["usageWindows"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|w| {
                Value::record([
                    ("label", Value::from(w["label"].as_str().unwrap_or_default())),
                    ("used", Value::from(w["usedFraction"].as_f64().unwrap_or(0.).clamp(0., 1.))),
                ])
            })
            .collect();
        let mut fields = vec![
            ("id", Value::from(account["id"].as_str().unwrap_or_default())),
            ("email", Value::from(account_name(account))),
            ("plan", Value::from(account["planLabel"].as_str().unwrap_or(""))),
            ("active", Value::Bool(account["active"].as_bool().unwrap_or(false))),
            (
                "can_switch",
                Value::Bool(account["switchable"].as_bool().unwrap_or(false) && zeron_proto::providers::switches_accounts(id)),
            ),
            ("windows", Value::List(windows.into())),
        ];
        if let Some(error) = account["usageError"].as_str() {
            fields.push(("error", Value::from(error)));
        }
        by.entry(harness).or_default().push(Value::record(fields));
    }
    Value::record(by.into_iter().map(|(harness, rows)| (harness, Value::List(rows.into()))))
}

/// What an account goes by.
fn account_name(account: &Json) -> &str {
    account["email"].as_str().or_else(|| account["displayName"].as_str()).unwrap_or("Signed in")
}

/// An RFC 3339 time ("2026-11-01T15:45:00.5Z", "…+01:00") as seconds since
/// the epoch.
fn epoch_secs(s: &str) -> Option<i64> {
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if s.get(4..5) != Some("-") || !matches!(s.get(10..11), Some("T" | "t" | " ")) {
        return None;
    }
    // Past the seconds: a fraction, then Z or an offset.
    let rest = s.get(19..)?;
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.get(0..1)? {
                "+" => 1,
                "-" => -1,
                _ => return None,
            };
            let oh: i64 = rest.get(1..3)?.parse().ok()?;
            let om: i64 = rest.get(4..6)?.parse().ok()?;
            sign * (oh * 3600 + om * 60)
        }
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + sec - offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_switch_moves_only_its_own_login_group() {
        let mut accounts = vec![
            json!({ "id": "a", "harness": "claude-code", "provider": null, "active": true }),
            json!({ "id": "b", "harness": "claude-code", "provider": null, "active": false }),
            json!({ "id": "c", "harness": "codex", "provider": null, "active": true }),
            json!({ "id": "d", "harness": "cursor", "provider": "x", "active": true }),
            json!({ "id": "e", "harness": "cursor", "provider": "y", "active": false }),
            json!({ "id": "f", "harness": "cursor", "provider": "y", "active": true }),
        ];
        mark_switched(&mut accounts, "claude-code", "b");
        mark_switched(&mut accounts, "cursor", "e");
        let active: Vec<&str> =
            accounts.iter().filter(|a| a["active"] == true).map(|a| a["id"].as_str().unwrap()).collect();
        assert_eq!(active, ["b", "c", "d", "e"]);
        // (An account it doesn't have: nothing moves.)
        mark_switched(&mut accounts, "claude-code", "nobody");
        assert_eq!(accounts[1]["active"], true);
    }

    #[test]
    fn a_device_lists_only_agents_that_sign_in_and_report_usage() {
        let value = device_accounts_value(&[
            json!({ "id": "a", "harness": "claude-code", "email": "a@example.test", "active": true, "switchable": true,
                "planLabel": "Max", "usageWindows": [{ "label": "Week", "usedFraction": 1.4 }] }),
            json!({ "id": "b", "harness": "antigravity", "active": true, "switchable": true }),
            json!({ "id": "c", "harness": "hermes", "active": true, "switchable": true, "usageError": "no" }),
            json!({ "id": "d", "harness": "acp.someone", "active": true }),
        ]);
        let Value::Map(_) = &value else { panic!("a record") };
        let rows = value.field("claude-code").expect("claude").list().to_vec();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].field("email").map(text).as_deref(), Some("a@example.test"));
        assert!(rows[0].field("can_switch").is_some_and(Value::truthy));
        let windows = rows[0].field("windows").unwrap().list().to_vec();
        assert_eq!(windows[0].field("used").and_then(Value::as_f64), Some(1.0));
        assert!(value.field("antigravity").is_none());
        assert!(value.field("acp.someone").is_none());
        // Hermes rotates its own logins: listed, not switched.
        let hermes = value.field("hermes").expect("hermes").list().to_vec();
        assert!(!hermes[0].field("can_switch").is_some_and(Value::truthy));
        assert_eq!(hermes[0].field("error").map(text).as_deref(), Some("no"));
    }

    #[test]
    fn accounts_read_their_two_windows() {
        let account = json!({
            "id": "a1", "harness": "codex", "email": "a@example.com", "planLabel": "Pro",
            "active": true,
            "usageWindows": [
                { "label": "5h", "usedFraction": 0.25 },
                { "label": "Weekly", "usedFraction": 1.4 }
            ]
        });
        let value = account_value(&account, None);
        assert_eq!(value.field("session").and_then(Value::as_f64), Some(0.25));
        assert_eq!(value.field("week").and_then(Value::as_f64), Some(1.0));
        assert_eq!(value.field("plan").map(text).as_deref(), Some("Pro"));
        assert_eq!(value.field("session_resets"), None);
        let timed = account_value(&json!({
            "id": "a3", "usageWindows": [{ "label": "5h", "usedFraction": 1.0, "resetsAt": "2026-11-01T15:45:00Z" }]
        }), None);
        assert_eq!(timed.field("session_resets").and_then(Value::as_f64), Some(1_793_547_900.0));
        assert_eq!(epoch_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_secs("2026-11-01T16:45:00.250+01:00"), Some(1_793_547_900));
        assert_eq!(epoch_secs("2000-02-29T12:00:00-05:30"), Some(951_845_400));
        assert_eq!(epoch_secs("soon"), None);
        let bare = account_value(&json!({ "id": "a2", "active": false }), None);
        assert_eq!(bare.field("email").map(text).as_deref(), Some("Signed in"));
        assert_eq!(bare.field("week").and_then(Value::as_f64), Some(0.));
        // Only an account with a credit summary (Codex's) banks anything.
        assert_eq!(bare.field("banked"), None);
        assert_eq!(bare.field("resets"), None);
    }

    fn ids(value: &Value) -> Vec<String> {
        match value.field("resets") {
            Some(Value::List(list)) => list.iter().filter_map(|c| c.field("id").map(text)).collect(),
            _ => Vec::new(),
        }
    }

    /// A Codex account at its limit with two banked resets, as
    /// `ListAgentAccounts` sends it (crates/proto `CodexResetCredits`).
    fn codex(fetched_at: i64) -> Json {
        json!({
            "id": "codex:a", "harness": "codex", "email": "ada@example.com", "planLabel": "Plus",
            "active": true, "usageFetchedAt": fetched_at,
            "usageWindows": [
                { "label": "5h", "usedFraction": 1.0 },
                { "label": "Weekly", "usedFraction": 0.64 }
            ],
            "codexResetCredits": {
                "availableCount": 3,
                "credits": [
                    { "id": "late", "title": "Full reset (Weekly + 5 hr)", "description": null, "expiresAt": "2026-11-12T00:00:00Z" },
                    { "id": "undated", "title": "Full reset", "description": "Week and session", "expiresAt": null },
                    { "id": "soon", "title": "Full reset (Weekly + 5 hr)", "description": "Week and session", "expiresAt": "2026-11-04T00:00:00Z" }
                ]
            }
        })
    }

    #[test]
    fn codex_accounts_bank_resets_soonest_first() {
        let value = account_value(&codex(1000), None);
        assert_eq!(value.field("banked").and_then(Value::as_f64), Some(3.));
        assert_eq!(ids(&value), ["soon", "late", "undated"]);
        let Some(Value::List(list)) = value.field("resets") else { panic!("no resets") };
        assert_eq!(list[0].field("scope").map(text).as_deref(), Some("Week and session"));
        assert_eq!(list[0].field("expires").and_then(Value::as_f64), Some(1_793_750_400.));
        // (No description: no scope; no expiry: none said.)
        assert_eq!(list[1].field("scope"), None);
        assert_eq!(list[2].field("expires"), None);
        assert_eq!(value.field("reset"), None);
        // None banked: a count of 0, no credits.
        let none = account_value(&json!({ "id": "x", "codexResetCredits": { "availableCount": 0, "credits": [] } }), None);
        assert_eq!(none.field("banked").and_then(Value::as_f64), Some(0.));
        assert!(ids(&none).is_empty());
    }

    #[test]
    fn a_reset_shows_what_it_did_until_fresh_usage_lands() {
        let used = Reset { state: "reset", why: None, credit: "soon".into(), at: 2000, generation: 1 };
        // Probed before the reset: the windows started over, that one spent.
        let value = account_value(&codex(1000), Some(&used));
        assert_eq!(value.field("session").and_then(Value::as_f64), Some(0.));
        assert_eq!(value.field("week").and_then(Value::as_f64), Some(0.));
        assert_eq!(value.field("banked").and_then(Value::as_f64), Some(2.));
        assert_eq!(ids(&value), ["late", "undated"]);
        assert_eq!(value.field("reset").and_then(|r| r.field("state")).map(text).as_deref(), Some("reset"));
        // Probed since: the usage and credits as they are now.
        let fresh = account_value(&codex(3000), Some(&used));
        assert_eq!(fresh.field("session").and_then(Value::as_f64), Some(1.));
        assert_eq!(fresh.field("banked").and_then(Value::as_f64), Some(3.));
        // A failure changes nothing but says why.
        let failed = Reset { state: "failed", why: Some("mismatch"), credit: "soon".into(), at: 2000, generation: 2 };
        let value = account_value(&codex(1000), Some(&failed));
        assert_eq!(value.field("session").and_then(Value::as_f64), Some(1.));
        assert_eq!(value.field("banked").and_then(Value::as_f64), Some(3.));
        assert_eq!(value.field("reset").and_then(|r| r.field("why")).map(text).as_deref(), Some("mismatch"));
    }

    #[test]
    fn reset_outcomes_and_refusals_read_as_words() {
        assert_eq!(reset_state(Some("reset")), ("reset", None));
        assert_eq!(reset_state(Some("already_redeemed")), ("reset", None));
        assert_eq!(reset_state(Some("nothing_to_reset")), ("nothing", None));
        assert_eq!(reset_state(Some("no_credit")), ("gone", None));
        assert_eq!(reset_state(Some("something_new")), ("failed", Some("failed")));
        assert_eq!(reset_state(None), ("failed", Some("failed")));
        // The engine's refusals (crates/engine agent_accounts.rs).
        assert_eq!(reset_failure("Codex returned a different account. Refresh Accounts and try again."), "mismatch");
        assert_eq!(reset_failure("This Codex login could not be verified for reset redemption."), "signed_out");
        assert_eq!(reset_failure("This Codex login needs to be refreshed."), "signed_out");
        assert_eq!(reset_failure("This Codex account is no longer saved."), "missing");
        assert_eq!(reset_failure("offline"), "offline");
        assert_eq!(reset_failure("Could not use Codex reset: 500"), "failed");
    }

    #[test]
    fn idempotency_keys_are_fresh_uuids() {
        let (a, b) = (idempotency_key(), idempotency_key());
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
        assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn only_web_and_loopback_pages_open() {
        assert!(openable("https://claude.ai/oauth/authorize?x=1"));
        assert!(openable("http://localhost:1455/auth"));
        assert!(!openable("file:///etc/passwd"));
        assert!(!openable("javascript:alert(1)"));
    }
}

/// The device Providers shows (none: this Mac), for the registry's calls.
pub(crate) fn shown_target() -> Option<String> {
    AGENTS.with(|a| a.target.borrow().clone())
}

/// `params` addressed to the device Providers shows.
pub(crate) fn addressed_to_shown(params: Json) -> Json {
    addressed_to(shown_target().as_deref(), params)
}

/// Looks at the agents again (one was added or removed elsewhere).
pub(crate) fn reload() {
    let agents = AGENTS.with(Rc::clone);
    engine::get().spawn(async move { agents.load().await });
}

/// `params` addressed to `target` (`targetDeviceId`), or as they are for
/// this Mac's engine.
fn addressed_to(target: Option<&str>, mut params: Json) -> Json {
    if let (Some(target), Some(object)) = (target, params.as_object_mut()) {
        object.insert("targetDeviceId".into(), json!(target));
    }
    params
}

/// An agent's updates, as `WatchHarnessUpdates` says.
#[derive(Clone, PartialEq)]
struct Update {
    /// A newer version, ready to take.
    latest: Option<String>,
    policy: String,
    /// One being taken now.
    updating: bool,
}

impl Update {
    fn from(status: &Json) -> Self {
        let phase = status["phase"].as_str().unwrap_or_default();
        let ready = phase == "available" && status["canApply"].as_bool() != Some(false);
        Self {
            latest: if ready { status["latestVersion"].as_str().map(Into::into) } else { None },
            policy: status["policy"].as_str().unwrap_or("notify").into(),
            updating: matches!(phase, "waiting-for-idle" | "preparing" | "downloading" | "installing" | "verifying"),
        }
    }
}
