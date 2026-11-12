//! Plugin services: a plugin's headless half (`"service": "service.luau"`,
//! with the `service` permission), run by the engine on each device that
//! allowed the plugin, callable by the plugin's windows and agents from this
//! device or another (plugins/mod.rs routes those calls here).
//!
//! Lifecycle, per plugin:
//! - **On demand.** A service starts on its first call, tool call or timer,
//!   and once when its version is first seen (to learn its timers; it stops
//!   again at once when it has none).
//! - **Idle.** With nothing in flight for [`Config::idle`], it stops. Its
//!   timers stay with the host, which starts it again when one is due.
//! - **Updates.** A new version loads beside the old one, which goes on
//!   serving until the new one has loaded, then finishes its calls in
//!   flight and stops. A version that doesn't load fails calls with why.
//!
//! Failures are the plugin's business: each comes back with a reason (see
//! [`reason`]) and is never retried, except one case. A method the service
//! marked `safe` runs once more when the VM ended during the call (out of
//! memory, a crash, a stop), since running it twice does no harm.

pub mod commands;
pub mod vm;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};
use zeron_plugin_guard::net;

pub use vm::{DEFAULT_MEMORY_MB, Described, Failure, Kind, Limits};
use vm::{Done, Ops, Spec, Vm};

/// Why a call didn't get an answer from the service, as callers branch on.
pub mod reason {
    /// The plugin isn't installed on that device.
    pub const NOT_INSTALLED: &str = "not_installed";
    /// It's there but can't run (tampered, broken).
    pub const BROKEN: &str = "broken";
    /// That device hasn't allowed it (or its service) yet.
    pub const NOT_APPROVED: &str = "not_approved";
    /// Turned off on that device.
    pub const OFF: &str = "off";
    /// That version of the plugin has no service.
    pub const NO_SERVICE: &str = "no_service";
    /// That device has another version of it.
    pub const VERSION_MISMATCH: &str = "version_mismatch";
    /// Someone else's plugin by the same id is installed there.
    pub const OTHER_AUTHOR: &str = "other_author";
    /// Its service.luau failed to load there (the message says why).
    pub const NOT_LOADED: &str = "not_loaded";
    /// The service serves no method (or tool) by that name.
    pub const UNKNOWN_METHOD: &str = "unknown_method";
    /// The handler failed: error(), or an op it awaited was refused.
    pub const FAILED: &str = "failed";
    /// The service stopped during the call and the method isn't `safe`, so
    /// it wasn't run again (it may or may not have done its work).
    pub const INTERRUPTED: &str = "interrupted";
    /// No answer within the call's timeout.
    pub const TIMEOUT: &str = "timeout";
    /// That device can't be reached.
    pub const OFFLINE: &str = "offline";
    /// That device's engine is from before plugin services.
    pub const OLD_ENGINE: &str = "old_engine";
    /// The call itself is wrong (no method, a bad device).
    pub const BAD_CALL: &str = "bad_call";
}

/// A refused or failed call: a [`reason`] and words for a person.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub reason: &'static str,
    pub message: String,
}

impl Refusal {
    pub fn new(reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }

    /// `{ ok: false, reason, message }`, as `CallPluginService` answers.
    pub fn to_json(&self) -> Json {
        json!({ "ok": false, "reason": self.reason, "message": self.message })
    }
}

/// What should run for one plugin: from its entry while it's on here.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub plugin: String,
    pub name: String,
    pub version: String,
    pub hash: String,
    /// Its snapshot (immutable files).
    pub dir: PathBuf,
    /// Its service's file, relative to `dir`.
    pub entry: String,
    /// What this device allowed it.
    pub allowed: Vec<String>,
    /// The programs its manifest names.
    pub commands: Vec<String>,
    /// The hosts its manifest names (`network.hosts`; none: any public).
    pub hosts: Vec<String>,
    /// The heap its manifest asks (`limits.memory_mb`; none: the config's).
    pub memory_mb: Option<u32>,
}

impl Plan {
    fn allows(&self, permission: &str) -> bool {
        self.allowed.iter().any(|p| p == permission)
    }
}

/// What the host lends services (plugins/mod.rs implements it).
pub trait Hooks: Send + Sync + 'static {
    /// `zeron.emit`: to the plugin's windows here and those watching.
    fn emit(&self, plugin: &str, event: &str, data: Json);
    /// `zeron.notify`, as `PostNotification` posts (an outcome word).
    fn notify(&self, plugin: &str, title: &str, body: &str, session: Option<&str>) -> String;
    /// This device's `{ id, name }`.
    fn device(&self) -> Json;
    /// `zeron.folders`: this device's chats' and projects' folders.
    fn folders(&self) -> Vec<String> {
        Vec::new()
    }
    /// Whether `plan`'s snapshot is still what was allowed, checked before
    /// each load (a changed one is never run).
    fn verify(&self, _plan: &Plan) -> Result<(), String> {
        Ok(())
    }
    /// `zeron.usage`, `zeron.switch_account`, `zeron.set_model`, …: asked
    /// of the engine for `plugin`, which checks what this device allowed it
    /// (plugins/agents). `done` with the answer, or why not.
    fn agents(&self, _plugin: &str, _args: Json, done: Done) {
        done(Err("Usage, accounts and models aren't available here".into()));
    }
    /// The `zeron.on` listeners `plugin`'s service registered, as it loaded:
    /// the engine tells each its events (`Kind::Event`).
    fn listeners(&self, _plugin: &str, _listeners: &[(String, String, Json)]) {}
}

#[derive(Debug, Clone)]
pub struct Config {
    /// A service with nothing in flight this long stops.
    pub idle: Duration,
    /// How often the host looks at each service (idle, timers).
    pub tick: Duration,
    /// How long a service may take to load.
    pub load_timeout: Duration,
    pub limits: Limits,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(5 * 60),
            tick: Duration::from_secs(5),
            load_timeout: Duration::from_secs(10),
            limits: Limits::default(),
        }
    }
}

/// A call's timeout unless it asks another, and the most it may ask.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
pub const MAX_TIMEOUT: Duration = Duration::from_secs(300);
/// What `zeron.emit` sends at once, at most (its JSON).
pub const MAX_EMIT_BYTES: usize = 64 * 1024;
/// A service's log lines kept for `PluginStatus`.
const LOG_LINES: usize = 50;
/// What a fetch answers with, at most.
const MAX_FETCH_BYTES: usize = 16 * 1024 * 1024;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The engine's plugin services. Cheap to clone.
#[derive(Clone)]
pub struct Services {
    inner: Arc<Shared>,
}

struct Shared {
    config: Config,
    store_dir: PathBuf,
    hooks: OnceLock<Arc<dyn Hooks>>,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
    closed: AtomicBool,
}

/// One plugin's service, across its VMs.
struct Slot {
    plugin: String,
    state: Mutex<SlotState>,
    /// One start at a time.
    start: tokio::sync::Mutex<()>,
    log: Mutex<VecDeque<String>>,
}

#[derive(Default)]
struct SlotState {
    plan: Option<Plan>,
    running: Option<Running>,
    /// What it serves, for the hash it said so for.
    described: Option<(String, Described)>,
    /// Why that hash's service didn't load.
    failed: Option<(String, String)>,
    timers: Vec<Timer>,
    /// A new version is loading: the old one serves meanwhile.
    swapping: bool,
    supervisor: Option<tokio::task::JoinHandle<()>>,
    /// VMs started (status, tests).
    starts: u32,
}

#[derive(Clone)]
struct Running {
    plan: Plan,
    vm: Vm,
    described: Described,
}

struct Timer {
    key: String,
    period: Duration,
    due: Instant,
}

impl Slot {
    fn note(&self, line: String) {
        let mut log = lock(&self.log);
        if log.len() >= LOG_LINES {
            log.pop_front();
        }
        log.push_back(line);
    }
}

impl Services {
    /// Services whose `zeron.store` files live in `store_dir`.
    pub fn new(store_dir: &Path, config: Config) -> Self {
        Self {
            inner: Arc::new(Shared {
                config,
                store_dir: store_dir.to_path_buf(),
                hooks: OnceLock::new(),
                slots: Mutex::new(HashMap::new()),
                closed: AtomicBool::new(false),
            }),
        }
    }

    pub fn set_hooks(&self, hooks: Arc<dyn Hooks>) {
        let _ = self.inner.hooks.set(hooks);
    }

    fn hooks(&self) -> Option<Arc<dyn Hooks>> {
        self.inner.hooks.get().cloned()
    }

    /// `zeron.store`'s file for `plugin`.
    pub fn store_path(store_dir: &Path, plugin: &str) -> PathBuf {
        store_dir.join(format!("{plugin}.service.json"))
    }

    fn slot(&self, plugin: &str) -> Arc<Slot> {
        lock(&self.inner.slots)
            .entry(plugin.to_owned())
            .or_insert_with(|| {
                Arc::new(Slot {
                    plugin: plugin.to_owned(),
                    state: Mutex::new(SlotState::default()),
                    start: tokio::sync::Mutex::new(()),
                    log: Mutex::new(VecDeque::new()),
                })
            })
            .clone()
    }

    fn current(&self, slot: &Arc<Slot>) -> bool {
        lock(&self.inner.slots)
            .get(&slot.plugin)
            .is_some_and(|s| Arc::ptr_eq(s, slot))
    }

    /// What should run now (after each scan): services not in `plans` stop;
    /// a changed plan loads its version beside the running one; a version
    /// not seen yet loads once to learn its timers.
    pub fn sync(&self, plans: Vec<Plan>) {
        if self.inner.closed.load(Ordering::SeqCst) {
            return;
        }
        let gone: Vec<Arc<Slot>> = {
            let mut slots = lock(&self.inner.slots);
            let wanted: Vec<&str> = plans.iter().map(|p| p.plugin.as_str()).collect();
            let gone: Vec<String> = slots
                .keys()
                .filter(|id| !wanted.contains(&id.as_str()))
                .cloned()
                .collect();
            gone.iter().filter_map(|id| slots.remove(id)).collect()
        };
        for slot in gone {
            retire(&slot, "it's off, removed or no longer allowed here");
        }
        for plan in plans {
            let slot = self.slot(&plan.plugin);
            let load = {
                let mut st = lock(&slot.state);
                if st.plan.as_ref() == Some(&plan) {
                    false
                } else {
                    if st.failed.as_ref().is_some_and(|(hash, _)| *hash != plan.hash) {
                        st.failed = None;
                    }
                    st.plan = Some(plan.clone());
                    match &st.running {
                        Some(running) => running.plan != plan,
                        None => st.described.as_ref().is_none_or(|(hash, _)| *hash != plan.hash),
                    }
                }
            };
            self.supervise(&slot);
            if load {
                self.load_beside(slot, plan);
            }
        }
    }

    /// Load `plan`'s VM in the background: the running one (if any) serves
    /// until it has loaded.
    fn load_beside(&self, slot: Arc<Slot>, plan: Plan) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let this = self.clone();
        runtime.spawn(async move {
            let swap = {
                let mut st = lock(&slot.state);
                let swap = st.running.is_some();
                st.swapping = swap;
                swap
            };
            let loaded = this.ensure(&slot, &plan, false).await;
            let mut st = lock(&slot.state);
            st.swapping = false;
            // Only learning its timers (nothing ran before, nothing's in
            // flight): with none, it stops again at once.
            if let Ok(running) = loaded
                && !swap
                && running.described.timers.is_empty()
                && !running.vm.status().busy()
                && st.running.as_ref().is_some_and(|r| r.vm.same(&running.vm))
            {
                st.running = None;
                running.vm.stop();
            }
        });
    }

    /// The running VM for `plan`, started if need be. A caller (`serve_old`)
    /// is served by the old version while a new one loads; the loader isn't.
    async fn ensure(&self, slot: &Arc<Slot>, plan: &Plan, serve_old: bool) -> Result<Running, Refusal> {
        let usable = |st: &SlotState, swapping_ok: bool| -> Option<Running> {
            st.running
                .as_ref()
                .filter(|r| !r.vm.status().ended() && (r.plan == *plan || (swapping_ok && st.swapping)))
                .cloned()
        };
        let not_loaded = |why: &str| {
            Refusal::new(reason::NOT_LOADED, format!("{}'s service didn't load: {why}", plan.name))
        };
        {
            let st = lock(&slot.state);
            if let Some(running) = usable(&st, serve_old) {
                return Ok(running);
            }
            if let Some((_, why)) = st.failed.as_ref().filter(|(hash, _)| *hash == plan.hash) {
                return Err(not_loaded(why));
            }
        }
        let _one = slot.start.lock().await;
        {
            let st = lock(&slot.state);
            if let Some(running) = usable(&st, false) {
                return Ok(running);
            }
            if let Some((_, why)) = st.failed.as_ref().filter(|(hash, _)| *hash == plan.hash) {
                return Err(not_loaded(why));
            }
        }
        let Some(hooks) = self.hooks() else {
            return Err(Refusal::new(reason::NOT_LOADED, "Plugin services aren't ready yet"));
        };
        if let Err(why) = hooks.verify(plan) {
            slot.note(format!("not loaded: {why}"));
            return Err(not_loaded(&why));
        }
        let ops = Arc::new(PluginOps {
            plan: plan.clone(),
            hooks: hooks.clone(),
            http: match guarded_client(&plan.hosts) {
                Ok(http) => http,
                Err(why) => return Err(not_loaded(&why)),
            },
            runtime: tokio::runtime::Handle::current(),
            slot: Arc::downgrade(slot),
        });
        let spec = Spec {
            plugin: plan.plugin.clone(),
            dir: plan.dir.clone(),
            entry: plan.entry.clone(),
            device: hooks.device(),
            store: Some(Self::store_path(&self.inner.store_dir, &plan.plugin)),
            limits: Limits {
                memory: plan
                    .memory_mb
                    .map_or(self.inner.config.limits.memory, |mb| (mb as usize) << 20),
                ..self.inner.config.limits.clone()
            },
            eval: plan.allows("eval"),
        };
        let (vm, loaded) = Vm::start(spec, ops);
        let outcome = match tokio::time::timeout(self.inner.config.load_timeout, loaded).await {
            Ok(Ok(Ok(described))) => Ok(described),
            Ok(Ok(Err(why))) => Err(Some(why)),
            Ok(Err(_)) => Err(Some("it crashed while loading".to_owned())),
            Err(_) => Err(None),
        };
        let described = match outcome {
            Ok(described) => described,
            Err(why) => {
                vm.stop();
                let mut st = lock(&slot.state);
                // Not the old version either: it's no longer what's installed.
                if let Some(old) = st.running.take_if(|r| r.plan != *plan) {
                    old.vm.stop();
                }
                return Err(match why {
                    Some(why) => {
                        slot.note(format!("didn't load: {why}"));
                        st.failed = Some((plan.hash.clone(), why.clone()));
                        not_loaded(&why)
                    }
                    None => not_loaded(&format!(
                        "it took over {} s",
                        self.inner.config.load_timeout.as_secs()
                    )),
                });
            }
        };
        if !self.current(slot) || self.inner.closed.load(Ordering::SeqCst) {
            vm.stop();
            return Err(Refusal::new(reason::OFF, format!("{} was turned off here", plan.name)));
        }
        let running = Running {
            plan: plan.clone(),
            vm,
            described: described.clone(),
        };
        let old = {
            let mut st = lock(&slot.state);
            st.starts += 1;
            st.failed = None;
            st.described = Some((plan.hash.clone(), described.clone()));
            let now = Instant::now();
            let timers = described
                .timers
                .iter()
                .map(|(key, seconds)| {
                    let period = Duration::from_secs_f64(seconds.max(30.0));
                    let due = st
                        .timers
                        .iter()
                        .find(|t| t.key == *key && t.period == period)
                        .map_or(now + period, |t| t.due);
                    Timer {
                        key: key.clone(),
                        period,
                        due,
                    }
                })
                .collect();
            st.timers = timers;
            st.running.replace(running.clone())
        };
        slot.note(format!("started ({} {})", plan.name, plan.version));
        hooks.listeners(&plan.plugin, &described.listeners);
        // The old version finishes its calls in flight, then stops.
        if let Some(old) = old {
            old.vm.stop();
        }
        self.supervise(slot);
        Ok(running)
    }

    /// Call the service `plan` runs: a method, a tool or a timer.
    pub async fn call(
        &self,
        plan: &Plan,
        kind: Kind,
        name: &str,
        args: Json,
        call: Json,
        timeout: Duration,
    ) -> Result<Json, Refusal> {
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(Refusal::new(reason::OFF, "Zeron is stopping"));
        }
        let slot = self.slot(&plan.plugin);
        {
            let mut st = lock(&slot.state);
            if st.plan.is_none() {
                st.plan = Some(plan.clone());
            }
        }
        self.supervise(&slot);
        let unknown = || {
            let what = if kind == Kind::Tool { "tool" } else { "method" };
            Refusal::new(reason::UNKNOWN_METHOD, format!("{}'s service has no {what} {name}", plan.name))
        };
        for attempt in 0..2 {
            let running = self.ensure(&slot, plan, true).await?;
            let safe = match kind {
                Kind::Method => *running.described.methods.get(name).ok_or_else(unknown)?,
                Kind::Tool if !running.described.tools.iter().any(|t| t == name) => return Err(unknown()),
                Kind::Event if !running.described.listeners.iter().any(|l| l.0 == name) => return Err(unknown()),
                Kind::Tool | Kind::Timer | Kind::Event => false,
            };
            let reply = running.vm.call(kind, name, args.clone(), call.clone());
            match tokio::time::timeout(timeout, reply).await {
                Ok(Ok(Ok(value))) => return Ok(value),
                Ok(Ok(Err(Failure::Error(why)))) => return Err(Refusal::new(reason::FAILED, why)),
                Ok(Ok(Err(Failure::Unknown))) => return Err(unknown()),
                Ok(Err(_)) => {
                    // It ended before answering: the next ensure starts another.
                    {
                        let mut st = lock(&slot.state);
                        if let Some(dead) = st.running.take_if(|r| r.vm.same(&running.vm)) {
                            dead.vm.stop();
                        }
                    }
                    if safe && attempt == 0 {
                        slot.note(format!("stopped during {name}; running it again (it's safe)"));
                        continue;
                    }
                    return Err(Refusal::new(
                        reason::INTERRUPTED,
                        if safe {
                            format!("{}'s service stopped during {name}, twice", plan.name)
                        } else {
                            format!(
                                "{}'s service stopped during {name}. It isn't marked safe, so it wasn't run again: it may or may not have done its work",
                                plan.name
                            )
                        },
                    ));
                }
                Err(_) => {
                    return Err(Refusal::new(
                        reason::TIMEOUT,
                        format!("{}'s service didn't answer {name} within {} s", plan.name, timeout.as_secs()),
                    ));
                }
            }
        }
        unreachable!("the loop returns")
    }

    /// The tools `plan`'s service answers (starting it if need be).
    pub async fn tools(&self, plan: &Plan) -> Result<Vec<String>, Refusal> {
        let slot = self.slot(&plan.plugin);
        Ok(self.ensure(&slot, plan, true).await?.described.tools)
    }

    /// One looker per slot: stops it when idle, fires its timers.
    fn supervise(&self, slot: &Arc<Slot>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mut st = lock(&slot.state);
        if st.supervisor.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        let (weak_slot, weak_shared) = (Arc::downgrade(slot), Arc::downgrade(&self.inner));
        st.supervisor = Some(runtime.spawn(supervisor(weak_slot, weak_shared)));
    }

    /// How `plugin`'s service stands (`PluginStatus`), or null without one.
    pub fn status(&self, plugin: &str) -> Json {
        let Some(slot) = lock(&self.inner.slots).get(plugin).cloned() else {
            return Json::Null;
        };
        let st = lock(&slot.state);
        let running = st.running.as_ref().filter(|r| !r.vm.status().ended());
        let described = running.map(|r| &r.described).or(st.described.as_ref().map(|(_, d)| d));
        let mut out = json!({
            "running": running.is_some(),
            "starts": st.starts,
            "log": lock(&slot.log).iter().collect::<Vec<_>>(),
        });
        if let Some(running) = running {
            out["version"] = json!(running.plan.version);
            out["busy"] = json!(running.vm.status().busy());
        }
        if let Some(described) = described {
            out["methods"] = json!(described.methods);
            out["tools"] = json!(described.tools);
            out["timers"] = json!(
                described
                    .timers
                    .iter()
                    .map(|(key, seconds)| json!({ "key": key, "seconds": seconds }))
                    .collect::<Vec<_>>()
            );
            if !described.listeners.is_empty() {
                out["listeners"] = json!(
                    described
                        .listeners
                        .iter()
                        .map(|(key, event, opts)| json!({ "key": key, "event": event, "opts": opts }))
                        .collect::<Vec<_>>()
                );
            }
        }
        if let Some((_, why)) = &st.failed {
            out["error"] = json!(why);
        }
        out
    }

    /// Stop every service (Zeron quitting).
    pub fn shutdown(&self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        let slots: Vec<Arc<Slot>> = lock(&self.inner.slots).drain().map(|(_, s)| s).collect();
        for slot in slots {
            retire(&slot, "Zeron is stopping");
        }
    }
}

/// Stop a slot's VM and its looker for good.
fn retire(slot: &Arc<Slot>, why: &str) {
    let mut st = lock(&slot.state);
    if let Some(running) = st.running.take() {
        running.vm.stop();
        slot.note(format!("stopped: {why}"));
    }
    if let Some(task) = st.supervisor.take() {
        task.abort();
    }
}

async fn supervisor(slot: Weak<Slot>, shared: Weak<Shared>) {
    loop {
        let (Some(slot), Some(shared)) = (slot.upgrade(), shared.upgrade()) else {
            return;
        };
        let services = Services { inner: shared };
        if !services.current(&slot) {
            return;
        }
        let config = services.inner.config.clone();
        let now = Instant::now();
        let (fire, plan, next) = {
            let mut st = lock(&slot.state);
            if let Some(running) = &st.running {
                let status = running.vm.status();
                if status.ended() {
                    st.running = None;
                } else if !status.busy() && status.idle_for() >= config.idle {
                    running.vm.stop();
                    st.running = None;
                    slot.note("stopped (idle)".into());
                }
            }
            let mut fire = Vec::new();
            for timer in st.timers.iter_mut() {
                if timer.due <= now {
                    fire.push(timer.key.clone());
                    timer.due = now + timer.period;
                }
            }
            let next = st.timers.iter().map(|t| t.due).min();
            (fire, st.plan.clone(), next)
        };
        if let Some(plan) = plan {
            for key in fire {
                let (services, plan, slot) = (services.clone(), plan.clone(), slot.clone());
                tokio::spawn(async move {
                    let call = json!({ "kind": "timer", "from": services.hooks().map(|h| h.device()) });
                    if let Err(refusal) = services
                        .call(&plan, Kind::Timer, &key, Json::Null, call, MAX_TIMEOUT)
                        .await
                    {
                        slot.note(format!("timer {key} failed: {}", refusal.message));
                    }
                });
            }
        }
        let wake = next.map_or(config.tick, |at| at.saturating_duration_since(now).min(config.tick));
        drop(slot);
        drop(services);
        tokio::time::sleep(wake.max(Duration::from_millis(10))).await;
    }
}

/// The world, as one plugin's permissions on this device let it reach it.
struct PluginOps {
    plan: Plan,
    hooks: Arc<dyn Hooks>,
    http: reqwest::Client,
    runtime: tokio::runtime::Handle,
    slot: Weak<Slot>,
}

impl Ops for PluginOps {
    fn start(&self, kind: &str, args: Json, done: Done) {
        let name = &self.plan.name;
        match kind {
            "run" => {
                if !self.plan.allows("commands") {
                    return done(Err(format!("{name} wasn't allowed to run programs on this device")));
                }
                let (allowed, hooks) = (self.plan.commands.clone(), self.hooks.clone());
                self.runtime.spawn_blocking(move || {
                    // A git that commits or pushes in one of this device's
                    // chats' or projects' folders runs that checkout's hooks
                    // (pre-commit, pre-push), as git does for you; anywhere
                    // else, or reading, none.
                    let trusted = |dir: &Path| hooks.folders().iter().any(|f| dir.starts_with(f));
                    done(commands::run_trusting(&args, &allowed, &trusted))
                });
            }
            "fetch" => {
                if !self.plan.allows("network") {
                    return done(Err(format!("{name} wasn't allowed to connect to the internet")));
                }
                let (http, policy) = (self.http.clone(), self.policy());
                self.runtime.spawn(async move { done(fetch(&http, &policy, &args).await) });
            }
            "folders" => {
                // (Where its chats are: as much as "sessions.read" says.)
                if !self.plan.allows("sessions.read") {
                    return done(Err(format!("{name} wasn't allowed to see your chats")));
                }
                let hooks = self.hooks.clone();
                self.runtime.spawn_blocking(move || done(Ok(json!(hooks.folders()))));
            }
            "notify" => {
                if !self.plan.allows("notifications") {
                    return done(Err(format!("{name} wasn't allowed to send you notifications")));
                }
                let title = args["title"].as_str().unwrap_or_default().trim().to_owned();
                if title.is_empty() {
                    return done(Err("A notification needs a title".into()));
                }
                let (hooks, plugin) = (self.hooks.clone(), self.plan.plugin.clone());
                // Said as the plugin's: "Git: …".
                let title = format!("{name}: {title}");
                self.runtime.spawn_blocking(move || {
                    let body = args["body"].as_str().unwrap_or_default();
                    let session = args["session"].as_str().filter(|s| !s.is_empty());
                    done(Ok(json!(hooks.notify(&plugin, &title, body, session))))
                });
            }
            "agents" => {
                // (Each op's permission is the engine's to check: agents.rs.)
                let (hooks, plugin) = (self.hooks.clone(), self.plan.plugin.clone());
                let _runtime = self.runtime.enter();
                hooks.agents(&plugin, args, done);
            }
            other => done(Err(format!("zeron.{other} isn't something a service can do"))),
        }
    }

    fn watch(&self, folders: Vec<PathBuf>, heard: vm::Heard) -> Result<Box<dyn Send>, String> {
        if !self.plan.allows("files.read") {
            return Err(format!("{} wasn't allowed to read files on this device", self.plan.name));
        }
        let heard = Arc::new(heard);
        let mut handles = Vec::new();
        for (place, folder) in folders.into_iter().enumerate() {
            if !folder.is_absolute() {
                return Err(format!("zeron.watch: \"{}\" isn't a whole path", folder.display()));
            }
            if !folder.is_dir() {
                return Err(format!("zeron.watch: there's no folder at \"{}\"", folder.display()));
            }
            // (Events name the folder as the system knows it: /private/tmp
            // for /tmp.)
            let root = std::fs::canonicalize(&folder).unwrap_or(folder.clone());
            let heard = heard.clone();
            handles.push(crate::fs_watch::FsWatchHub::global().watch(
                &folder,
                notify::RecursiveMode::Recursive,
                move |event: &notify::Event| {
                    if !crate::plugins::is_change(event) {
                        return;
                    }
                    if event.need_rescan() || event.paths.is_empty() {
                        return heard(vm::Seen::All(place));
                    }
                    for path in &event.paths {
                        match path.strip_prefix(&root) {
                            Ok(rest) => {
                                let parts: Vec<String> =
                                    rest.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                                heard(vm::Seen::Path(place, parts.join("/")));
                            }
                            Err(_) => heard(vm::Seen::All(place)),
                        }
                    }
                },
            ));
        }
        Ok(Box::new(handles))
    }

    fn emit(&self, event: &str, data: Json) {
        let size = serde_json::to_vec(&data).map(|b| b.len()).unwrap_or(usize::MAX);
        if event.is_empty() || size > MAX_EMIT_BYTES {
            self.log(&format!("zeron.emit(\"{event}\") dropped: an event's name, and 64 KB of data at most"));
            return;
        }
        // (On the VM's thread: the host sends to other devices on the runtime.)
        let _runtime = self.runtime.enter();
        self.hooks.emit(&self.plan.plugin, event, data);
    }

    fn log(&self, line: &str) {
        tracing::info!(plugin = %self.plan.plugin, "plugins: service: {line}");
        if let Some(slot) = self.slot.upgrade() {
            slot.note(line.to_owned());
        }
    }
}

impl PluginOps {
    fn policy(&self) -> net::Policy {
        // (Checked when the manifest was: a bad list never gets here.)
        net::Policy::new(&self.plan.hosts).unwrap_or_default()
    }
}

/// A URL a plugin's request may go to (and each redirect's): http(s), a
/// host its manifest allows, an address literal that's public or named.
fn check_url(policy: &net::Policy, url: &reqwest::Url) -> Result<(), String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("zeron.fetch reaches http and https addresses only".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("zeron.fetch doesn't take credentials in an address".into());
    }
    let port = url.port_or_known_default().unwrap_or(0);
    // (As the URL parser wrote it: `0x7f.1` is `127.0.0.1` by now.)
    let name = url.host_str().ok_or("That address has no host")?.to_owned();
    let ip = name.trim_start_matches('[').trim_end_matches(']').parse::<std::net::IpAddr>().ok();
    policy.host(&name, port)?;
    // (A name is checked as it resolves: `GuardedResolver`.)
    if let Some(ip) = ip {
        policy.address(&name, port, ip)?;
    }
    Ok(())
}

/// Names resolved, then each address checked before anything connects to
/// it: a name that resolves (or later rebinds) to this device, your
/// network or instance metadata never reaches it.
struct GuardedResolver {
    policy: net::Policy,
}

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let policy = self.policy.clone();
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let found: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            let mut refused = None;
            let allowed: Vec<std::net::SocketAddr> = found
                .into_iter()
                .filter(|addr| match policy.resolved(&host, addr.ip()) {
                    Ok(()) => true,
                    Err(why) => {
                        refused = Some(why);
                        false
                    }
                })
                .collect();
            if allowed.is_empty() {
                let why = refused.unwrap_or_else(|| format!("{host} has no address"));
                return Err(why.into());
            }
            Ok(Box::new(allowed.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// A plugin's HTTP client: its hosts' rules at every connection and
/// redirect, and never a proxy from the environment.
fn guarded_client(hosts: &[String]) -> Result<reqwest::Client, String> {
    let policy = net::Policy::new(hosts)?;
    let redirects = policy.clone();
    reqwest::Client::builder()
        .no_proxy()
        .dns_resolver(Arc::new(GuardedResolver { policy }))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 10 {
                return attempt.error("too many redirects");
            }
            match check_url(&redirects, attempt.url()) {
                Ok(()) => attempt.follow(),
                Err(why) => attempt.error(why),
            }
        }))
        .build()
        .map_err(|e| format!("The network isn't available: {e}"))
}

/// `zeron.fetch`: `{ url, method?, headers?, body?, timeout? }` → `{ status,
/// headers, body }` (http and https only; the body as text, 16 MB at most).
/// Where it may go is plugin-guard's `net` (R15): public addresses, or the
/// hosts the manifest names; checked again on every redirect and at every
/// address a name resolves to.
async fn fetch(http: &reqwest::Client, policy: &net::Policy, args: &Json) -> Result<Json, String> {
    let url = args["url"].as_str().unwrap_or_default();
    let parsed = reqwest::Url::parse(url).map_err(|_| format!("\"{url}\" isn't a web address"))?;
    check_url(policy, &parsed)?;
    let (host, port) = (parsed.host_str().unwrap_or_default().to_owned(), parsed.port_or_known_default().unwrap_or(0));
    let method = args["method"].as_str().unwrap_or("GET").to_ascii_uppercase();
    let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| format!("\"{method}\" isn't an HTTP method"))?;
    let timeout = args["timeout"]
        .as_f64()
        .filter(|s| *s > 0.0)
        .map_or(DEFAULT_TIMEOUT, |s| Duration::from_secs_f64(s).min(MAX_TIMEOUT));
    let mut request = http.request(method, parsed).timeout(timeout);
    for (key, value) in args["headers"].as_object().into_iter().flatten() {
        if let Some(value) = value.as_str() {
            policy.header(&host, port, key)?;
            request = request.header(key.as_str(), value);
        }
    }
    if let Some(body) = args["body"].as_str() {
        request = request.body(body.to_owned());
    }
    let mut response = request.send().await.map_err(|e| {
        // (Say the rule that stopped it, not reqwest's wrapping of it.)
        let mut source: Option<&dyn std::error::Error> = Some(&e);
        let mut deepest = e.to_string();
        while let Some(s) = source {
            deepest = s.to_string();
            source = s.source();
        }
        format!("The request failed: {deepest}")
    })?;
    let status = response.status().as_u16();
    let headers: serde_json::Map<String, Json> = response
        .headers()
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_owned(), json!(v))))
        .collect();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| format!("The response broke off: {e}"))? {
        if body.len() + chunk.len() > MAX_FETCH_BYTES {
            return Err("The response is over 16 MB".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(json!({ "status": status, "headers": headers, "body": String::from_utf8_lossy(&body) }))
}

#[cfg(test)]
mod tests;
