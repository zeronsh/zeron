//! What plugins may know and do about the agents: their accounts' usage,
//! the saved accounts and which is active, the models a harness offers and
//! a chat's model; and the events of those changing (`zeron.on`).
//!
//! **The engine decides.** Every op and every event is checked here against
//! what this device allowed the plugin (`Sink::grants`), never against what
//! a window or a service says it may do:
//!
//! | permission | lets the plugin |
//! |---|---|
//! | `usage.read` | read each account's usage windows and plan; usage events |
//! | `accounts.read` | read the saved accounts (and their email); account events |
//! | `accounts.switch` | make another saved account the active one |
//! | `models.read` | read a harness's models and a chat's model; model events |
//! | `models.set` | set a chat's model, effort and options |
//!
//! Turn and input events need `sessions.read`. A field a plugin wasn't
//! allowed is left out of what it's told, never blanked.
//!
//! **Never** tokens, login flows, reset credits, or creating and forgetting
//! logins: those stay Settings'.
//!
//! **Fired on change.** The engine already learns each change (an accounts
//! list or switch, from Settings, the composer's usage ring or a plugin; a
//! session's status; a chat's config; a catalog listed; a turn's error):
//! each is compared with the last, and what changed is told. Nothing here
//! polls, and a plugin's read never forces a usage probe (a probe can
//! rotate a saved login's single-use refresh token): only `usage_refresh`
//! does, paced per plugin on top of the engine's own 30 s per account.
//!
//! **Controls are attributed**, not limited: each switch or model set is
//! logged as the plugin's and told with `by`.
//!
//! **A loop is stopped** (`loop_guard`): a plugin told of its own change
//! that answers with another, and again, [`LOOP_HOPS`] times in a row, has
//! that next change refused (`loop`), and its row in Settings says so. How
//! often a plugin changes things never counts, only its changes setting off
//! its own (as cloud functions stop a function re-triggering itself).
//!
//! **A failing listener stays**: each failure is logged, and its row in
//! Settings says the latest and how many in a row.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde_json::{Map, Value as Json, json};
use zeron_proto::{
    AgentAccount, AgentAccountsSnapshot, Chat, ChatConfig, DoneStatus, HarnessId, Model, ReasoningLevel,
    Session, SessionStatus, run_errors::ProviderProblem,
};

pub const USAGE_READ: &str = "usage.read";
pub const ACCOUNTS_READ: &str = "accounts.read";
pub const ACCOUNTS_SWITCH: &str = "accounts.switch";
pub const MODELS_READ: &str = "models.read";
pub const MODELS_SET: &str = "models.set";
const SESSIONS_READ: &str = "sessions.read";

/// Every event a plugin may listen for (`zeron.on`).
pub const EVENTS: &[&str] = &[
    "usage.changed",
    "usage.threshold",
    "usage.reset",
    "limit.hit",
    "account.switched",
    "account.signed_out",
    "login.expired",
    "model.changed",
    "models.updated",
    "turn.started",
    "turn.finished",
    "turn.errored",
    "chat.awaiting_input",
    "chat.input_answered",
];

/// What an event needs the plugin to have been allowed.
pub fn event_needs(event: &str) -> Option<&'static str> {
    Some(match event {
        "usage.changed" | "usage.threshold" | "usage.reset" | "limit.hit" => USAGE_READ,
        "account.switched" | "account.signed_out" | "login.expired" => ACCOUNTS_READ,
        "model.changed" | "models.updated" => MODELS_READ,
        "turn.started" | "turn.finished" | "turn.errored" | "chat.awaiting_input" | "chat.input_answered" => {
            SESSIONS_READ
        }
        _ => return None,
    })
}

/// Pacing (tests make them short).
#[derive(Debug, Clone)]
pub struct Pacing {
    /// Between two forced usage reads by one plugin.
    pub refresh_gap: Duration,
    /// How long a `when_idle` switch waits for that provider's chats.
    pub idle_wait: Duration,
}

impl Default for Pacing {
    fn default() -> Self {
        Self {
            refresh_gap: Duration::from_secs(5 * 60),
            idle_wait: Duration::from_secs(30 * 60),
        }
    }
}

/// Why an op was refused: a word callers branch on, and words for a person.
#[derive(Debug, Clone, PartialEq)]
pub struct Refused {
    pub reason: &'static str,
    pub message: String,
}

impl Refused {
    fn new(reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }

    /// `"<reason>: <words>"`, as a promise fails with it.
    pub fn text(&self) -> String {
        format!("{}: {}", self.reason, self.message)
    }
}

pub mod reason {
    /// This device didn't allow the plugin what the op needs.
    pub const NOT_ALLOWED: &str = "not_allowed";
    /// Too soon after the last usage refresh (see [`super::Pacing`]).
    pub const PACED: &str = "paced";
    /// It's that way already.
    pub const ALREADY: &str = "already";
    /// The plugin's own changes kept setting each other off (`LOOP_HOPS`).
    pub const LOOP: &str = "loop";
    /// No such provider, account, chat, model, effort or option.
    pub const NOT_FOUND: &str = "not_found";
    /// The op itself is wrong.
    pub const BAD_CALL: &str = "bad_call";
    /// Zeron tried and it failed (the words say why).
    pub const FAILED: &str = "failed";
}

/// The engine, as plugins' agent ops reach it (lib.rs implements it).
pub trait AgentHost: Send + Sync + 'static {
    /// `ListAgentAccounts` on this device (`force`: probe usage).
    fn accounts(&self, force: bool) -> BoxFuture<'static, Result<AgentAccountsSnapshot, String>>;
    /// `ActivateAgentAccount` on this device.
    fn activate(&self, harness: HarnessId, account: String) -> BoxFuture<'static, Result<AgentAccountsSnapshot, String>>;
    /// `ListModels` on this device.
    fn models(&self, harness: HarnessId) -> BoxFuture<'static, Result<Vec<Model>, String>>;
    /// The workspace's chats.
    fn chats(&self) -> Vec<Chat>;
    /// This device's sessions (their status).
    fn sessions(&self) -> Vec<Session>;
    /// `Mutate setChatConfig`.
    fn set_chat_config(&self, chat: &str, config: ChatConfig) -> Result<(), String>;
}

/// What a plugin was allowed on this device, while it's on.
#[derive(Debug, Clone)]
pub struct Grant {
    pub name: String,
    pub allowed: Vec<String>,
}

impl Grant {
    fn allows(&self, permission: &str) -> bool {
        self.allowed.iter().any(|p| p == permission)
    }
}

/// Where events go, and who may hear them (plugins/mod.rs implements it).
pub trait Sink: Send + Sync + 'static {
    /// What `plugin` was allowed here, if it's on.
    fn grants(&self, plugin: &str) -> Option<Grant>;
    /// To `plugin`'s windows on this device.
    fn to_window(&self, plugin: &str, listener: &str, event: &str, data: Json);
    /// To `plugin`'s service on this device (started if need be).
    fn to_service(&self, plugin: &str, listener: &str, event: &str, data: Json) -> BoxFuture<'static, Result<(), String>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Target {
    Window,
    Service,
}

#[derive(Debug, Clone)]
struct Listener {
    key: String,
    event: String,
    opts: Json,
}

/// Who made a change.
#[derive(Debug, Clone)]
struct By {
    plugin: String,
    name: String,
}

impl By {
    fn json(&self) -> Json {
        json!({ "plugin": self.plugin, "name": self.name })
    }
}

#[derive(Debug, Clone)]
struct PendingSwitch {
    account: String,
    by: By,
    until: Instant,
}

/// A chat's model as last seen.
type ChatModel = (Option<String>, Option<ReasoningLevel>, Map<String, Json>);

#[derive(Default)]
struct State {
    listeners: BTreeMap<(String, Target), Vec<Listener>>,
    /// Each threshold listener's side of its line, per account window.
    below: HashMap<(String, Target, String, String, String, String), bool>,
    accounts: Option<AgentAccountsSnapshot>,
    sessions: Option<HashMap<String, SessionStatus>>,
    outcomes: HashMap<String, Option<DoneStatus>>,
    chats: Option<HashMap<String, ChatModel>>,
    models: HashMap<String, Vec<String>>,
    /// The provider problem a chat's turn reported, until it ends.
    problems: HashMap<String, ProviderProblem>,
    refreshed_at: HashMap<String, Instant>,
    pending: HashMap<(String, String), PendingSwitch>,
    /// A switch or set a plugin is making: told with its `by`.
    switch_by: HashMap<String, (String, By)>,
    model_by: HashMap<String, By>,
    /// Each plugin's last actions (`plugin_status`).
    actions: HashMap<String, VecDeque<Json>>,
    /// What each plugin's row says (a listener failing, a loop stopped).
    notices: HashMap<String, VecDeque<String>>,
    /// A listener's failures in a row.
    failures: HashMap<(String, Target, String), u32>,
    /// When each plugin was last told of a change it made itself.
    echoed: HashMap<String, Instant>,
    /// Each plugin's changes in a row, each set off by its last.
    chain: HashMap<String, u32>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

const ACTIONS_KEPT: usize = 20;
const NOTICES_KEPT: usize = 5;

/// A plugin's changes, each set off by its last, before the next is refused.
pub const LOOP_HOPS: u32 = 16;
/// How soon after a plugin is told of its own change another counts as set
/// off by it.
const LOOP_WINDOW: Duration = Duration::from_secs(10);

/// Plugins' agent ops and events. Cheap to clone.
#[derive(Clone)]
pub struct Agents {
    inner: Arc<Inner>,
}

struct Inner {
    host: Mutex<Option<Arc<dyn AgentHost>>>,
    sink: Mutex<Option<Arc<dyn Sink>>>,
    pacing: Pacing,
    state: Mutex<State>,
}

/// One event before it's fitted to a plugin's grants: what it says, and the
/// fields only a further permission shows.
#[derive(Debug, Clone)]
struct Raw {
    event: &'static str,
    data: Map<String, Json>,
    extra: Vec<(&'static str, &'static str, Json)>,
}

impl Raw {
    fn new(event: &'static str, data: Json) -> Self {
        Self {
            event,
            data: data.as_object().cloned().unwrap_or_default(),
            extra: Vec::new(),
        }
    }

    fn with(mut self, permission: &'static str, field: &'static str, value: Json) -> Self {
        if !value.is_null() {
            self.extra.push((permission, field, value));
        }
        self
    }

    /// What `grant` may be told of it, or nothing.
    fn fitted(&self, grant: &Grant) -> Option<Json> {
        if !grant.allows(event_needs(self.event)?) {
            return None;
        }
        let mut data = self.data.clone();
        data.retain(|_, v| !v.is_null());
        for (permission, field, value) in &self.extra {
            if grant.allows(permission) {
                data.insert((*field).to_owned(), value.clone());
            }
        }
        Some(Json::Object(data))
    }
}

/// Whether `opts` (a listener's `provider`, `account`, `window`, `chat`)
/// lets `data` through.
fn matches(opts: &Json, data: &Json) -> bool {
    ["provider", "account", "window", "chat"].iter().all(|key| match opts.get(*key).and_then(Json::as_str) {
        None => true,
        Some(want) => data.get(*key).and_then(Json::as_str).is_none_or(|have| have.eq_ignore_ascii_case(want)),
    })
}

fn percent(fraction: f32) -> f64 {
    (f64::from(fraction.clamp(0.0, 1.0)) * 1000.0).round() / 10.0
}

fn window_json(w: &zeron_proto::AgentUsageWindow) -> Json {
    json!({
        "label": w.label,
        "used": percent(w.used_fraction),
        "remaining": ((100.0 - percent(w.used_fraction)) * 10.0).round() / 10.0,
        "resets_at": w.resets_at.map(|t| t.timestamp_millis()),
    })
}

/// An account's usage, as `usage.read` shows it (no email).
fn usage_row(a: &AgentAccount) -> Json {
    let mut row = json!({
        "provider": a.harness.as_str(),
        "account": a.id,
        "active": a.active,
        "windows": a.usage_windows.iter().map(window_json).collect::<Vec<_>>(),
    });
    if let Some(plan) = &a.plan_label {
        row["plan"] = json!(plan);
    }
    if let Some(at) = a.usage_fetched_at {
        row["fetched_at"] = json!(at);
    }
    if let Some(error) = &a.usage_error {
        row["error"] = json!(error);
    }
    row
}

/// An account, as `accounts.read` shows it (never a token, never a credit).
fn account_row(a: &AgentAccount) -> Json {
    let mut row = json!({
        "provider": a.harness.as_str(),
        "account": a.id,
        "active": a.active,
        "switchable": a.switchable,
    });
    for (key, value) in [
        ("email", &a.email),
        ("name", &a.display_name),
        ("organization", &a.organization),
        ("plan", &a.plan_label),
    ] {
        if let Some(v) = value {
            row[key] = json!(v);
        }
    }
    row
}

fn provider_arg(args: &Json, key: &str) -> Result<Option<HarnessId>, Refused> {
    match args.get(key).and_then(Json::as_str).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(slug) => HarnessId::from_slug(slug)
            .map(Some)
            .ok_or_else(|| Refused::new(reason::NOT_FOUND, format!("There's no provider \"{slug}\""))),
    }
}

fn effort_word(level: ReasoningLevel) -> String {
    serde_json::to_value(level)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn model_row(m: &Model) -> Json {
    json!({
        "id": m.id,
        "label": m.label,
        "description": m.description,
        "efforts": m.reasoning_levels.iter().map(|l| effort_word(*l)).collect::<Vec<_>>(),
        "options": m.options.iter().map(|o| json!({
            "id": o.id,
            "label": o.label,
            "default": o.default_choice,
            "choices": o.choices.iter().map(|c| json!({ "id": c.id, "label": c.label })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn chat_model_json(chat: &str, config: &ChatConfig) -> Json {
    json!({
        "chat": chat,
        "provider": config.harness.as_str(),
        "model": config.model,
        "effort": config.reasoning.map(effort_word),
        "options": config.model_options,
    })
}

fn active(snapshot: &AgentAccountsSnapshot, harness: HarnessId) -> Option<&AgentAccount> {
    snapshot.accounts.iter().find(|a| a.harness == harness && a.active)
}

impl Default for Agents {
    fn default() -> Self {
        Self::new(Pacing::default())
    }
}

impl Agents {
    pub fn new(pacing: Pacing) -> Self {
        Self {
            inner: Arc::new(Inner {
                host: Mutex::new(None),
                sink: Mutex::new(None),
                pacing,
                state: Mutex::new(State::default()),
            }),
        }
    }

    pub fn set_host(&self, host: Arc<dyn AgentHost>) {
        *lock(&self.inner.host) = Some(host);
    }

    pub fn set_sink(&self, sink: Arc<dyn Sink>) {
        *lock(&self.inner.sink) = Some(sink);
    }

    fn host(&self) -> Result<Arc<dyn AgentHost>, Refused> {
        lock(&self.inner.host)
            .clone()
            .ok_or_else(|| Refused::new(reason::FAILED, "Usage, accounts and models aren't available on this device"))
    }

    fn sink(&self) -> Option<Arc<dyn Sink>> {
        lock(&self.inner.sink).clone()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.inner.state)
    }

    // ── Ops ────────────────────────────────────────────────────────────────

    /// `plugin`'s op (`args.op`) on this device: its answer, or why not.
    pub async fn call(&self, plugin: &str, args: Json) -> Result<Json, Refused> {
        let grant = self
            .sink()
            .and_then(|s| s.grants(plugin))
            .ok_or_else(|| Refused::new(reason::NOT_ALLOWED, format!("{plugin} isn't on here")))?;
        let op = args.get("op").and_then(Json::as_str).unwrap_or_default().to_owned();
        let need = |permission: &str, what: &str| {
            if grant.allows(permission) {
                Ok(())
            } else {
                Err(Refused::new(
                    reason::NOT_ALLOWED,
                    format!("{} wasn't allowed to {what} on this device (it needs \"{permission}\")", grant.name),
                ))
            }
        };
        let by = By {
            plugin: plugin.to_owned(),
            name: grant.name.clone(),
        };
        match op.as_str() {
            "usage" => {
                need(USAGE_READ, "see your agents' usage")?;
                let provider = provider_arg(&args, "provider")?;
                let snapshot = self.read_accounts(false).await?;
                Ok(json!(
                    snapshot
                        .accounts
                        .iter()
                        .filter(|a| provider.is_none_or(|p| a.harness == p))
                        .map(usage_row)
                        .collect::<Vec<_>>()
                ))
            }
            "usage_refresh" => {
                need(USAGE_READ, "see your agents' usage")?;
                {
                    let mut st = self.state();
                    let now = Instant::now();
                    if let Some(at) = st.refreshed_at.get(plugin)
                        && now.duration_since(*at) < self.inner.pacing.refresh_gap
                    {
                        let wait = self.inner.pacing.refresh_gap - now.duration_since(*at);
                        return Err(Refused::new(
                            reason::PACED,
                            format!("Usage was read again for {} {} s ago; try in {} s", grant.name, now.duration_since(*at).as_secs(), wait.as_secs()),
                        ));
                    }
                    st.refreshed_at.insert(plugin.to_owned(), now);
                }
                let snapshot = self.read_accounts(true).await?;
                Ok(json!(snapshot.accounts.iter().map(usage_row).collect::<Vec<_>>()))
            }
            "accounts" => {
                need(ACCOUNTS_READ, "see your agents' accounts")?;
                let provider = provider_arg(&args, "provider")?;
                let snapshot = self.read_accounts(false).await?;
                Ok(json!(
                    snapshot
                        .accounts
                        .iter()
                        .filter(|a| provider.is_none_or(|p| a.harness == p))
                        .map(account_row)
                        .collect::<Vec<_>>()
                ))
            }
            "switch_account" => {
                need(ACCOUNTS_SWITCH, "switch your agents' accounts")?;
                self.switch(&by, &args).await
            }
            "models" => {
                need(MODELS_READ, "see your agents' models")?;
                let harness = provider_arg(&args, "harness")?
                    .ok_or_else(|| Refused::new(reason::BAD_CALL, "Say which harness's models"))?;
                let models = self.read_models(harness).await?;
                Ok(json!(models.iter().map(model_row).collect::<Vec<_>>()))
            }
            "chat_model" => {
                need(MODELS_READ, "see your agents' models")?;
                let chat = self.chat(&args)?;
                let config = chat
                    .config
                    .as_ref()
                    .ok_or_else(|| Refused::new(reason::NOT_FOUND, "That chat hasn't chosen a model yet"))?;
                Ok(chat_model_json(&chat.id, config))
            }
            "set_model" => {
                need(MODELS_SET, "change your chats' models")?;
                self.set_model(&by, &args).await
            }
            "set_default_model" => {
                // The default for new chats is the window's own (the app
                // keeps it): checked and told here, kept there.
                need(MODELS_SET, "change your chats' models")?;
                let harness = provider_arg(&args, "harness")?
                    .ok_or_else(|| Refused::new(reason::BAD_CALL, "Say which harness's default"))?;
                let model = args.get("model").and_then(Json::as_str).unwrap_or_default().to_owned();
                let models = self.read_models(harness).await?;
                let found = models
                    .iter()
                    .find(|m| m.id == model)
                    .ok_or_else(|| Refused::new(reason::NOT_FOUND, format!("{} has no model \"{model}\"", harness.as_str())))?;
                let effort = self.effort(found, &args)?;
                self.loop_guard(&by)?;
                let data = json!({
                    "default": true,
                    "provider": harness.as_str(),
                    "model": model,
                    "effort": effort.map(effort_word),
                    "by": by.json(),
                });
                self.record(&by, json!({ "op": "set_default_model", "provider": harness.as_str(), "model": model }));
                self.tell(vec![Raw::new("model.changed", data.clone())]);
                Ok(data)
            }
            "" => Err(Refused::new(reason::BAD_CALL, "Say which op")),
            other => Err(Refused::new(reason::BAD_CALL, format!("\"{other}\" isn't an op"))),
        }
    }

    async fn read_accounts(&self, force: bool) -> Result<AgentAccountsSnapshot, Refused> {
        let snapshot = self
            .host()?
            .accounts(force)
            .await
            .map_err(|why| Refused::new(reason::FAILED, why))?;
        self.observe_accounts(&snapshot);
        Ok(snapshot)
    }

    async fn read_models(&self, harness: HarnessId) -> Result<Vec<Model>, Refused> {
        let models = self
            .host()?
            .models(harness)
            .await
            .map_err(|why| Refused::new(reason::FAILED, why))?;
        self.observe_models(harness, &models);
        Ok(models)
    }

    fn chat(&self, args: &Json) -> Result<Chat, Refused> {
        let id = args.get("chat").and_then(Json::as_str).unwrap_or_default();
        if id.is_empty() {
            return Err(Refused::new(reason::BAD_CALL, "Say which chat"));
        }
        self.host()?
            .chats()
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| Refused::new(reason::NOT_FOUND, format!("There's no chat {id}")))
    }

    /// Whether a chat on `harness` runs on this device now.
    fn busy(&self, host: &dyn AgentHost, harness: HarnessId) -> bool {
        let chats: HashMap<String, HarnessId> = host
            .chats()
            .into_iter()
            .filter_map(|c| Some((c.id, c.config?.harness)))
            .collect();
        host.sessions().iter().any(|s| {
            matches!(s.status, SessionStatus::Working | SessionStatus::AwaitingInput)
                && chats.get(&s.chat_id) == Some(&harness)
        })
    }

    async fn switch(&self, by: &By, args: &Json) -> Result<Json, Refused> {
        let harness = provider_arg(args, "provider")?
            .ok_or_else(|| Refused::new(reason::BAD_CALL, "Say which provider's account"))?;
        let account = args.get("account").and_then(Json::as_str).unwrap_or_default().to_owned();
        if account.is_empty() {
            return Err(Refused::new(reason::BAD_CALL, "Say which account"));
        }
        let host = self.host()?;
        let snapshot = self.read_accounts(false).await?;
        let target = snapshot
            .accounts
            .iter()
            .find(|a| a.harness == harness && a.id == account)
            .ok_or_else(|| Refused::new(reason::NOT_FOUND, format!("There's no saved {} account {account}", harness.as_str())))?;
        if target.active {
            return Err(Refused::new(reason::ALREADY, "That account is the active one already"));
        }
        if !target.switchable {
            return Err(Refused::new(reason::FAILED, "Zeron can't switch to that account"));
        }
        self.loop_guard(by)?;
        // Switched at once, running chats or not; `when_idle` asks to wait
        // for that provider's chats here to finish first.
        if args.get("when_idle").and_then(Json::as_bool) == Some(true) && self.busy(host.as_ref(), harness) {
            self.state().pending.insert(
                (by.plugin.clone(), harness.as_str().to_owned()),
                PendingSwitch {
                    account: account.clone(),
                    by: by.clone(),
                    until: Instant::now() + self.inner.pacing.idle_wait,
                },
            );
            self.record(by, json!({ "op": "switch_account", "provider": harness.as_str(), "account": account, "waiting": true }));
            return Ok(json!({ "status": "waiting", "provider": harness.as_str(), "account": account }));
        }
        self.activate(by, harness, &account).await
    }

    async fn activate(&self, by: &By, harness: HarnessId, account: &str) -> Result<Json, Refused> {
        let host = self.host()?;
        self.state()
            .switch_by
            .insert(harness.as_str().to_owned(), (account.to_owned(), by.clone()));
        tracing::info!(plugin = %by.plugin, provider = harness.as_str(), "plugins: switching the active account");
        let snapshot = match host.activate(harness, account.to_owned()).await {
            Ok(snapshot) => snapshot,
            Err(why) => {
                self.state().switch_by.remove(harness.as_str());
                return Err(Refused::new(reason::FAILED, why));
            }
        };
        self.record(by, json!({ "op": "switch_account", "provider": harness.as_str(), "account": account }));
        self.observe_accounts(&snapshot);
        Ok(json!({ "status": "switched", "provider": harness.as_str(), "account": account }))
    }

    fn effort(&self, model: &Model, args: &Json) -> Result<Option<ReasoningLevel>, Refused> {
        let Some(word) = args.get("effort").and_then(Json::as_str).filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        model
            .reasoning_levels
            .iter()
            .copied()
            .find(|l| effort_word(*l) == word)
            .map(Some)
            .ok_or_else(|| Refused::new(reason::NOT_FOUND, format!("{} has no effort \"{word}\"", model.id)))
    }

    async fn set_model(&self, by: &By, args: &Json) -> Result<Json, Refused> {
        let chat = self.chat(args)?;
        let mut config = chat
            .config
            .clone()
            .ok_or_else(|| Refused::new(reason::NOT_FOUND, "That chat hasn't chosen a provider yet"))?;
        if let Some(asked) = args.get("provider").and_then(Json::as_str)
            && asked != config.harness.as_str()
        {
            return Err(Refused::new(reason::BAD_CALL, "A plugin changes a chat's model on its own provider only"));
        }
        let models = self.read_models(config.harness).await?;
        let model_id = args
            .get("model")
            .and_then(Json::as_str)
            .map(str::to_owned)
            .or_else(|| config.model.clone())
            .or_else(|| models.first().map(|m| m.id.clone()))
            .unwrap_or_default();
        let model = models.iter().find(|m| m.id == model_id).ok_or_else(|| {
            Refused::new(reason::NOT_FOUND, format!("{} has no model \"{model_id}\"", config.harness.as_str()))
        })?;
        let effort = self.effort(model, args)?;
        let mut options = config.model_options.clone();
        for (id, choice) in args.get("options").and_then(Json::as_object).into_iter().flatten() {
            let option = model
                .options
                .iter()
                .find(|o| o.id == *id)
                .ok_or_else(|| Refused::new(reason::NOT_FOUND, format!("{} has no option \"{id}\"", model.id)))?;
            let choice = choice.as_str().unwrap_or_default();
            if !option.choices.iter().any(|c| c.id == choice) {
                return Err(Refused::new(reason::NOT_FOUND, format!("{id} has no choice \"{choice}\"")));
            }
            options.insert(id.clone(), json!(choice));
        }
        let changed_model = config.model.as_deref() != Some(model.id.as_str());
        config.model = Some(model.id.clone());
        if let Some(effort) = effort {
            config.reasoning = Some(effort);
        } else if changed_model && config.reasoning.is_some_and(|r| !model.reasoning_levels.contains(&r)) {
            // An effort the new model doesn't have: its own start.
            config.reasoning = None;
        }
        config.model_options = options;
        if chat.config.as_ref() == Some(&config) {
            return Err(Refused::new(reason::ALREADY, "That chat has that model already"));
        }
        self.loop_guard(by)?;
        let host = self.host()?;
        self.state().model_by.insert(chat.id.clone(), by.clone());
        if let Err(why) = host.set_chat_config(&chat.id, config.clone()) {
            self.state().model_by.remove(&chat.id);
            return Err(Refused::new(reason::FAILED, why));
        }
        tracing::info!(plugin = %by.plugin, chat = %chat.id, "plugins: set a chat's model");
        self.record(by, json!({ "op": "set_model", "chat": chat.id, "model": config.model }));
        // (The chat's row changing tells model.changed, with `by`.)
        Ok(json!({ "status": "set", "applies": "next_turn", "model": chat_model_json(&chat.id, &config) }))
    }

    fn record(&self, by: &By, mut action: Json) {
        action["at"] = json!(crate::now_ms());
        let mut st = self.state();
        let kept = st.actions.entry(by.plugin.clone()).or_default();
        if kept.len() >= ACTIONS_KEPT {
            kept.pop_front();
        }
        kept.push_back(action);
    }

    /// What `plugin_status` says: its last actions, and what its row says.
    pub fn status(&self, plugin: &str) -> Json {
        let st = self.state();
        let actions: Vec<Json> = st.actions.get(plugin).map(|a| a.iter().cloned().collect()).unwrap_or_default();
        let notices: Vec<String> = st.notices.get(plugin).map(|n| n.iter().cloned().collect()).unwrap_or_default();
        if actions.is_empty() && notices.is_empty() {
            return Json::Null;
        }
        json!({ "actions": actions, "notices": notices })
    }

    /// What `plugin`'s row in Settings says, latest last.
    pub fn notices(&self, plugin: &str) -> Vec<String> {
        self.state().notices.get(plugin).map(|n| n.iter().cloned().collect()).unwrap_or_default()
    }

    /// Kept for `plugin_status`, and told to the plugin's window, which
    /// reports it as a problem of its own (its row in Settings → Plugins).
    fn notice(&self, plugin: &str, notice: String) {
        {
            let mut st = self.state();
            let kept = st.notices.entry(plugin.to_owned()).or_default();
            if kept.len() >= NOTICES_KEPT {
                kept.pop_front();
            }
            kept.push_back(notice.clone());
        }
        if let Some(sink) = self.sink() {
            sink.to_window(plugin, "@notice", "plugin.notice", json!({ "notice": notice }));
        }
    }

    /// Before a plugin's change: refused when its own changes have set each
    /// other off `LOOP_HOPS` times in a row. A change not set off by its own
    /// starts the count again.
    fn loop_guard(&self, by: &By) -> Result<(), Refused> {
        let now = Instant::now();
        let mut st = self.state();
        let set_off = st.echoed.get(&by.plugin).is_some_and(|at| now.duration_since(*at) < LOOP_WINDOW);
        let hops = if set_off { st.chain.get(&by.plugin).copied().unwrap_or(0) + 1 } else { 1 };
        if hops > LOOP_HOPS {
            st.chain.remove(&by.plugin);
            st.echoed.remove(&by.plugin);
            drop(st);
            tracing::warn!(plugin = %by.plugin, "plugins: stopped a loop of a plugin's own changes");
            self.notice(
                &by.plugin,
                format!("Stopped a loop: {} changes in a row, each set off by its own last one", LOOP_HOPS),
            );
            return Err(Refused::new(
                reason::LOOP,
                format!("{}'s changes kept setting each other off ({LOOP_HOPS} in a row): this one wasn't made", by.name),
            ));
        }
        st.chain.insert(by.plugin.clone(), hops);
        Ok(())
    }

    // ── Listeners ──────────────────────────────────────────────────────────

    /// `plugin`'s listeners at `target` (all of them, replacing those it
    /// had): `(key, event, opts)`. An event it can't hear is kept (its
    /// grants may change) but never told.
    pub fn listen(&self, plugin: &str, target: Target, listeners: &[(String, String, Json)]) {
        let mut st = self.state();
        let key = (plugin.to_owned(), target);
        let listeners: Vec<Listener> = listeners
            .iter()
            .filter(|(_, event, _)| EVENTS.contains(&event.as_str()))
            .map(|(key, event, opts)| Listener {
                key: key.clone(),
                event: event.clone(),
                opts: if opts.is_object() { opts.clone() } else { json!({}) },
            })
            .collect();
        if listeners.is_empty() {
            st.listeners.remove(&key);
        } else {
            st.listeners.insert(key, listeners);
        }
        st.below.retain(|(p, t, ..), _| !(p == plugin && *t == target));
        st.failures.retain(|(p, t, _), _| !(p == plugin && *t == target));
        // A threshold's baseline is what's known now.
        let accounts = st.accounts.clone();
        drop(st);
        if let Some(snapshot) = accounts {
            self.thresholds(&snapshot, true);
        }
    }

    /// Forget every plugin but `keep` (after a scan: off, removed).
    pub fn retain(&self, keep: &HashSet<String>) {
        let mut st = self.state();
        st.listeners.retain(|(p, _), _| keep.contains(p));
        st.pending.retain(|(p, _), _| keep.contains(p));
    }

    /// Tell each listener what it may hear of `events`.
    fn tell(&self, events: Vec<Raw>) {
        if events.is_empty() {
            return;
        }
        let Some(sink) = self.sink() else { return };
        let listeners: Vec<((String, Target), Vec<Listener>)> =
            self.state().listeners.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for ((plugin, target), list) in listeners {
            let Some(grant) = sink.grants(&plugin) else { continue };
            for raw in &events {
                if raw.event == "usage.threshold" {
                    continue; // (Each listener's own: `thresholds`.)
                }
                let Some(data) = raw.fitted(&grant) else { continue };
                // Its own change, told back to it: what `loop_guard` counts.
                let own = raw.data.get("by").and_then(|b| b.get("plugin")).and_then(Json::as_str) == Some(plugin.as_str());
                let mut told = false;
                for listener in list.iter().filter(|l| l.event == raw.event && matches(&l.opts, &data)) {
                    self.deliver(&sink, &plugin, target, &listener.key, raw.event, data.clone());
                    told = true;
                }
                if own && told {
                    self.state().echoed.insert(plugin.clone(), Instant::now());
                }
            }
        }
    }

    fn deliver(&self, sink: &Arc<dyn Sink>, plugin: &str, target: Target, key: &str, event: &str, data: Json) {
        match target {
            Target::Window => sink.to_window(plugin, key, event, data),
            Target::Service => {
                let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
                let told = sink.to_service(plugin, key, event, data);
                let (this, plugin, key, event) = (self.clone(), plugin.to_owned(), key.to_owned(), event.to_owned());
                // Never awaited by whoever changed something: a handler that's
                // slow or fails slows nothing, and fails only itself.
                runtime.spawn(async move {
                    let outcome = told.await;
                    this.heard(&plugin, target, &key, &event, outcome);
                });
            }
        }
    }

    /// How a listener took an event. A failure is logged and said on the
    /// plugin's row (how many in a row); the listener stays, and is told the
    /// next event as before.
    pub fn heard(&self, plugin: &str, target: Target, key: &str, event: &str, outcome: Result<(), String>) {
        let id = (plugin.to_owned(), target, key.to_owned());
        match outcome {
            Ok(()) => {
                self.state().failures.remove(&id);
            }
            Err(why) => {
                let n = {
                    let mut st = self.state();
                    let n = st.failures.entry(id).or_default();
                    *n += 1;
                    *n
                };
                tracing::warn!(%plugin, %event, %why, failures = n, "plugins: a listener failed");
                let times = if n == 1 { String::new() } else { format!(" ({n} times in a row)") };
                self.notice(plugin, format!("Its {event} listener failed{times}: {why}"));
            }
        }
    }

    // ── What changed ───────────────────────────────────────────────────────

    /// An accounts snapshot, from whoever read it (Settings, the composer's
    /// ring, a plugin, a switch): what changed since the last is told.
    pub fn observe_accounts(&self, next: &AgentAccountsSnapshot) {
        let prev = {
            let mut st = self.state();
            if st.accounts.as_ref() == Some(next) {
                return;
            }
            st.accounts.replace(next.clone())
        };
        let mut events = Vec::new();
        if let Some(prev) = &prev {
            let key = |a: &AgentAccount| (a.harness, a.id.clone());
            let before: HashMap<_, &AgentAccount> = prev.accounts.iter().map(|a| (key(a), a)).collect();
            let after: HashMap<_, &AgentAccount> = next.accounts.iter().map(|a| (key(a), a)).collect();
            for a in &next.accounts {
                let Some(was) = before.get(&key(a)) else { continue };
                if was.usage_windows != a.usage_windows || was.plan_label != a.plan_label {
                    events.push(
                        Raw::new("usage.changed", usage_row(a)).with(ACCOUNTS_READ, "email", json!(a.email)),
                    );
                }
                for w in &a.usage_windows {
                    let Some(old) = was.usage_windows.iter().find(|o| o.label == w.label) else { continue };
                    let advanced = matches!((old.resets_at, w.resets_at), (Some(o), Some(n)) if n > o);
                    if (advanced && w.used_fraction < old.used_fraction) || old.used_fraction - w.used_fraction >= 0.5 {
                        events.push(Raw::new(
                            "usage.reset",
                            json!({ "provider": a.harness.as_str(), "account": a.id, "window": w.label, "next_reset_at": w.resets_at.map(|t| t.timestamp_millis()) }),
                        ));
                    }
                    if old.used_fraction < 1.0 && w.used_fraction >= 1.0 {
                        events.push(Raw::new(
                            "limit.hit",
                            json!({ "provider": a.harness.as_str(), "account": a.id, "window": w.label, "resets_at": w.resets_at.map(|t| t.timestamp_millis()) }),
                        ));
                    }
                }
                let signed_out = |e: &Option<String>| {
                    e.as_deref().is_some_and(|e| {
                        zeron_proto::run_errors::provider_problem(e) == Some(ProviderProblem::SignedOut)
                            || e.to_ascii_lowercase().contains("sign in")
                    })
                };
                if !signed_out(&was.usage_error) && signed_out(&a.usage_error) {
                    events.push(Raw::new("login.expired", json!({ "provider": a.harness.as_str(), "account": a.id })));
                }
            }
            for (k, a) in &before {
                if !after.contains_key(k) {
                    events.push(Raw::new("account.signed_out", json!({ "provider": a.harness.as_str(), "account": a.id })));
                }
            }
            let harnesses: HashSet<HarnessId> = next.accounts.iter().map(|a| a.harness).collect();
            for harness in harnesses {
                let (from, to) = (active(prev, harness), active(next, harness));
                let Some(to) = to else { continue };
                if from.is_some_and(|f| f.id == to.id) {
                    continue;
                }
                let by = {
                    let mut st = self.state();
                    match st.switch_by.remove(harness.as_str()) {
                        Some((account, by)) if account == to.id => Some(by),
                        _ => None,
                    }
                };
                let mut data = json!({ "provider": harness.as_str(), "account": to.id, "from": from.map(|f| f.id.clone()) });
                if let Some(by) = by {
                    data["by"] = by.json();
                }
                events.push(
                    Raw::new("account.switched", data)
                        .with(ACCOUNTS_READ, "email", json!(to.email))
                        .with(ACCOUNTS_READ, "from_email", json!(from.and_then(|f| f.email.clone()))),
                );
            }
        }
        self.tell(events);
        self.thresholds(next, prev.is_none());
    }

    /// Each threshold listener's line, for the accounts in `snapshot`: told
    /// once as remaining crosses under it, again only after it's back over.
    /// `baseline`: only learn which side each is on.
    fn thresholds(&self, snapshot: &AgentAccountsSnapshot, baseline: bool) {
        let Some(sink) = self.sink() else { return };
        let mut tell = Vec::new();
        {
            let mut st = self.state();
            let listeners: Vec<((String, Target), Listener)> = st
                .listeners
                .iter()
                .flat_map(|(k, v)| v.iter().filter(|l| l.event == "usage.threshold").map(move |l| (k.clone(), l.clone())))
                .collect();
            for ((plugin, target), listener) in listeners {
                let Some(below) = listener.opts.get("below").and_then(Json::as_f64) else { continue };
                let Some(grant) = sink.grants(&plugin) else { continue };
                if !grant.allows(USAGE_READ) {
                    continue;
                }
                let account = listener.opts.get("account").and_then(Json::as_str);
                for a in &snapshot.accounts {
                    // The active account, unless it names one.
                    if account.map_or(!a.active, |id| id != a.id) {
                        continue;
                    }
                    for w in &a.usage_windows {
                        let remaining = 100.0 - percent(w.used_fraction);
                        let mut data = json!({
                            "provider": a.harness.as_str(),
                            "account": a.id,
                            "window": w.label,
                            "remaining": (remaining * 10.0).round() / 10.0,
                            "below": below,
                            "resets_at": w.resets_at.map(|t| t.timestamp_millis()),
                        });
                        if !matches(&listener.opts, &data) {
                            continue;
                        }
                        let id = (plugin.clone(), target, listener.key.clone(), a.harness.as_str().to_owned(), a.id.clone(), w.label.clone());
                        let now_below = remaining < below;
                        let was = st.below.insert(id, now_below);
                        if !baseline && was == Some(false) && now_below {
                            if grant.allows(ACCOUNTS_READ)
                                && let Some(email) = &a.email
                            {
                                data["email"] = json!(email);
                            }
                            tell.push((plugin.clone(), target, listener.key.clone(), data));
                        }
                    }
                }
            }
        }
        for (plugin, target, key, data) in tell {
            self.deliver(&sink, &plugin, target, &key, "usage.threshold", data);
        }
    }

    /// This device's sessions, as they change: turns starting and ending,
    /// a chat asking and being answered.
    pub fn observe_sessions(&self, sessions: &[Session]) {
        let mut events = Vec::new();
        let provider_of = |chat: &str| -> Option<String> {
            let host = lock(&self.inner.host).clone()?;
            host.chats().into_iter().find(|c| c.id == chat)?.config.map(|c| c.harness.as_str().to_owned())
        };
        let mut settled = false;
        {
            let mut st = self.state();
            let first = st.sessions.is_none();
            let mut prev = st.sessions.take().unwrap_or_default();
            let mut next = HashMap::new();
            for s in sessions {
                let was = prev.remove(&s.chat_id);
                next.insert(s.chat_id.clone(), s.status);
                st.outcomes.insert(s.chat_id.clone(), s.last_turn_outcome.map(|o| o.status));
                if first {
                    continue;
                }
                let active = |x: Option<SessionStatus>| matches!(x, Some(SessionStatus::Working | SessionStatus::AwaitingInput));
                let chat = json!({ "chat": s.chat_id });
                match (was, s.status) {
                    (w, SessionStatus::Working) if !active(w) => {
                        st.problems.remove(&s.chat_id);
                        events.push(("turn.started", s.chat_id.clone(), chat));
                    }
                    (Some(SessionStatus::Working), SessionStatus::AwaitingInput) => {
                        events.push(("chat.awaiting_input", s.chat_id.clone(), chat));
                    }
                    (w, SessionStatus::AwaitingInput) if !active(w) => {
                        events.push(("chat.awaiting_input", s.chat_id.clone(), chat));
                    }
                    (Some(SessionStatus::AwaitingInput), SessionStatus::Working) => {
                        events.push(("chat.input_answered", s.chat_id.clone(), chat));
                    }
                    (w, SessionStatus::Idle | SessionStatus::Errored) if active(w) => {
                        settled = true;
                        let problem = st.problems.remove(&s.chat_id);
                        let outcome = if problem == Some(ProviderProblem::LimitReached) {
                            "limited"
                        } else if s.status == SessionStatus::Errored {
                            "errored"
                        } else {
                            match s.last_turn_outcome.map(|o| o.status) {
                                Some(DoneStatus::Interrupted) => "interrupted",
                                Some(DoneStatus::Errored) => "errored",
                                _ => "completed",
                            }
                        };
                        let mut data = json!({ "chat": s.chat_id, "outcome": outcome });
                        if problem == Some(ProviderProblem::SignedOut) {
                            data["problem"] = json!("signed_out");
                        }
                        events.push(("turn.finished", s.chat_id.clone(), data.clone()));
                        if outcome == "errored" || outcome == "limited" {
                            events.push(("turn.errored", s.chat_id.clone(), data));
                        }
                    }
                    _ => {}
                }
            }
            if !prev.is_empty() {
                settled = true;
            }
            st.sessions = Some(next);
        }
        let raws = events
            .into_iter()
            .map(|(event, chat, data)| Raw::new(event, data).with(MODELS_READ, "provider", json!(provider_of(&chat))))
            .collect();
        self.tell(raws);
        if settled {
            self.run_pending();
        }
    }

    /// A turn's error, as a harness said it: a limit, a sign-in.
    pub fn observe_problem(&self, chat: &str, message: &str) {
        let Some(problem) = zeron_proto::run_errors::provider_problem(message) else { return };
        {
            let mut st = self.state();
            if st.problems.get(chat) == Some(&problem) {
                return;
            }
            st.problems.insert(chat.to_owned(), problem);
        }
        let provider = lock(&self.inner.host)
            .clone()
            .and_then(|h| h.chats().into_iter().find(|c| c.id == chat))
            .and_then(|c| c.config.map(|c| c.harness.as_str().to_owned()));
        let event = match problem {
            ProviderProblem::LimitReached => "limit.hit",
            ProviderProblem::SignedOut => "login.expired",
        };
        self.tell(vec![
            Raw::new(event, json!({ "provider": provider, "blocked_turn": true })).with(SESSIONS_READ, "chat", json!(chat)),
        ]);
    }

    /// The workspace's chats, as they change: a chat's model, effort or
    /// options changing is told (with `by` when a plugin set it).
    pub fn observe_chats(&self, chats: &[Chat]) {
        let mut events = Vec::new();
        {
            let mut st = self.state();
            let first = st.chats.is_none();
            let prev = st.chats.take().unwrap_or_default();
            let mut next = HashMap::new();
            for chat in chats {
                let Some(config) = &chat.config else { continue };
                let now: ChatModel = (config.model.clone(), config.reasoning, config.model_options.clone());
                if !first && prev.get(&chat.id).is_some_and(|was| *was != now) {
                    let mut data = chat_model_json(&chat.id, config);
                    if let Some(by) = st.model_by.remove(&chat.id) {
                        data["by"] = by.json();
                    }
                    events.push(Raw::new("model.changed", data));
                }
                next.insert(chat.id.clone(), now);
            }
            st.chats = Some(next);
        }
        self.tell(events);
    }

    /// A harness's catalog, as it was listed: told when its models change.
    pub fn observe_models(&self, harness: HarnessId, models: &[Model]) {
        let ids: Vec<String> = models.iter().map(|m| m.id.clone()).collect();
        let changed = {
            let mut st = self.state();
            match st.models.insert(harness.as_str().to_owned(), ids.clone()) {
                Some(was) => was != ids,
                None => false,
            }
        };
        if changed {
            self.tell(vec![Raw::new("models.updated", json!({ "provider": harness.as_str(), "models": ids }))]);
        }
    }

    /// The `when_idle` switches whose provider has nothing running now.
    fn run_pending(&self) {
        let Ok(host) = self.host() else { return };
        let now = Instant::now();
        let due: Vec<(String, String, PendingSwitch)> = {
            let mut st = self.state();
            st.pending.retain(|_, p| p.until > now);
            st.pending.iter().map(|((pl, h), p)| (pl.clone(), h.clone(), p.clone())).collect()
        };
        for (plugin, provider, pending) in due {
            let Some(harness) = HarnessId::from_slug(&provider) else { continue };
            if self.busy(host.as_ref(), harness) {
                continue;
            }
            self.state().pending.remove(&(plugin.clone(), provider.clone()));
            let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
            let this = self.clone();
            runtime.spawn(async move {
                // Still allowed. (Its loop was counted as it asked.)
                let still = this.sink().and_then(|s| s.grants(&plugin)).is_some_and(|g| g.allows(ACCOUNTS_SWITCH));
                if !still {
                    return;
                }
                if let Err(refused) = this.activate(&pending.by, harness, &pending.account).await {
                    tracing::info!(%plugin, reason = refused.reason, "plugins: a waiting switch didn't happen");
                }
            });
        }
    }
}

/// The engine's feeds into `agents`: account snapshots, sessions and
/// chats as they change. Each lasts while `agents` does.
pub fn follow(
    agents: &Agents,
    mut accounts: tokio::sync::watch::Receiver<Option<AgentAccountsSnapshot>>,
    mut sessions: tokio::sync::watch::Receiver<Vec<Session>>,
    mut chats: tokio::sync::watch::Receiver<Vec<Chat>>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return Vec::new();
    };
    let weak: Weak<Inner> = Arc::downgrade(&agents.inner);
    let up = move |w: &Weak<Inner>| w.upgrade().map(|inner| Agents { inner });
    let (w1, w2, w3) = (weak.clone(), weak.clone(), weak);
    vec![
        runtime.spawn(async move {
            loop {
                let snapshot = accounts.borrow_and_update().clone();
                if let (Some(snapshot), Some(agents)) = (snapshot, up(&w1)) {
                    agents.observe_accounts(&snapshot);
                }
                if accounts.changed().await.is_err() {
                    return;
                }
            }
        }),
        runtime.spawn(async move {
            loop {
                let list = sessions.borrow_and_update().clone();
                match up(&w2) {
                    Some(agents) => agents.observe_sessions(&list),
                    None => return,
                }
                if sessions.changed().await.is_err() {
                    return;
                }
            }
        }),
        runtime.spawn(async move {
            loop {
                let list = chats.borrow_and_update().clone();
                match up(&w3) {
                    Some(agents) => agents.observe_chats(&list),
                    None => return,
                }
                if chats.changed().await.is_err() {
                    return;
                }
            }
        }),
    ]
}

#[cfg(test)]
#[path = "agents_tests.rs"]
pub(crate) mod tests;
