//! HarnessRegistry — the engine's harness catalog: eager instances (mock) plus lazy
//! slots resolved on first use (claude-code spawns subprocess discovery; codex/cursor
//! later). Lazy slots carry a static descriptor so `ListHarnesses` never forces a spawn.
//!
//! Also owns the device's harness ENABLEMENT (Settings → Providers): which harnesses
//! this device's composer offers, persisted in `{data_dir}/harness-prefs.json`.
//! Per-device because CLI installs are — a viewer retargets the settings page at
//! another device and edits THAT device's set over the forwarded RPCs.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

use zeron_harness::{Harness, HarnessError, mock::MockHarness};
use zeron_proto::{AgentEvent, DoneStatus, HarnessId, ReasoningLevel, SteeringMode};

/// What `ListHarnesses` reports per harness.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessDescriptor {
    pub id: HarnessId,
    pub name: String,
    pub supports_steering: bool,
    pub steering_mode: SteeringMode,
    pub reasoning_levels: Vec<ReasoningLevel>,
    /// Whether the agent's CLI is present on the listing device (the settings
    /// enable-gate). Defaults true so catalogs from engines predating the
    /// field never read as uninstallable.
    #[serde(default = "default_installed")]
    pub installed: bool,
    /// Explicit CLI installation is available on this listing device.
    #[serde(default)]
    pub can_install: bool,
    /// Whether the listing device offers this harness (Settings → Providers).
    /// `None` — the catalog came from an engine predating the setting — means
    /// "unknown": consumers fall back to detection (see [`descriptor_enabled`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

impl HarnessDescriptor {
    /// Whether this harness can accept a prompt inside the turn that is
    /// currently running. Turn-boundary steering is still useful to the
    /// automatic queue drain, but it is not the non-interrupting "Steer"
    /// action exposed on an individual queued row.
    pub fn steers_mid_turn(&self) -> bool {
        self.supports_steering && self.steering_mode == SteeringMode::StepBoundary
    }
}

fn default_installed() -> bool {
    true
}

/// Whether detection alone may switch a harness on. The mock always "resolves"
/// but is a test rig, and counting it would let the last real agent be turned
/// off (the composer needs something it can actually run); the dev rig opts it
/// in through `COMET_HARNESS=mock`, which bypasses enablement in the pickers.
fn auto_enabled(id: HarnessId) -> bool {
    id != HarnessId::Mock
}

/// A descriptor's effective enabled flag. `None` — a catalog from an engine
/// predating the setting — falls back to detection, the same rule new devices
/// start from (see [`HarnessRegistry::enabled_set`]).
pub fn descriptor_enabled(descriptor: &HarnessDescriptor) -> bool {
    descriptor
        .enabled
        .unwrap_or_else(|| descriptor.installed && auto_enabled(descriptor.id))
}

fn describe(harness: &dyn Harness) -> HarnessDescriptor {
    HarnessDescriptor {
        id: harness.id(),
        name: harness.display_name().to_string(),
        supports_steering: harness.supports_steering(),
        steering_mode: harness.steering_mode(),
        reasoning_levels: harness.reasoning_levels().to_vec(),
        installed: harness.installed(),
        can_install: false,
        enabled: None,
    }
}

/// The persisted shape of `harness-prefs.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct HarnessPrefsFile {
    /// The user's explicit opt-OUTS. Enablement otherwise follows detection,
    /// so the file only records "no" — an agent installed later turns itself
    /// on without a trip to Settings.
    disabled: Vec<HarnessId>,
    titles: TitleSettings,
    /// The allow-list written back when enablement was a fixed default set.
    /// Read once, folded into `disabled`, and never written again.
    #[serde(skip_serializing)]
    enabled: Option<Vec<HarnessId>>,
}

/// Per-device automatic session title preferences.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TitleSettings {
    /// None follows the session harness, using a supported installed fallback.
    pub harness: Option<HarnessId>,
    /// None selects the cheapest model offered by the selected harness.
    pub model: Option<String>,
}

type Factory = Box<dyn Fn() -> Result<Arc<dyn Harness>, HarnessError> + Send + Sync>;
type InstalledProbe = Box<dyn Fn() -> bool + Send + Sync>;

enum Slot {
    Ready(Arc<dyn Harness>),
    Lazy {
        descriptor: HarnessDescriptor,
        /// Re-run on every `descriptors()` call — a CLI installed mid-session
        /// shows up on the next settings/picker open, no restart needed.
        installed: InstalledProbe,
        factory: Factory,
    },
}

type CatalogKey = (HarnessId, PathBuf);

/// Last-known slash-command and skill lists. Entries are small, errors are
/// never stored, and `generation` lets an in-flight refresh notice that its
/// harness was updated, disabled, or replaced out from under it.
struct CatalogCache {
    commands: HashMap<CatalogKey, Vec<zeron_proto::SlashCommand>>,
    skills: HashMap<CatalogKey, Option<Vec<zeron_proto::invocation::Skill>>>,
    refreshing_commands: HashSet<CatalogKey>,
    refreshing_skills: HashSet<CatalogKey>,
    generation: HashMap<HarnessId, u64>,
}

impl CatalogCache {
    fn generation_of(&self, id: HarnessId) -> u64 {
        self.generation.get(&id).copied().unwrap_or(0)
    }

    fn invalidate(&mut self, id: HarnessId) {
        let next = self.generation_of(id).wrapping_add(1);
        self.generation.insert(id, next);
        self.commands.retain(|(harness, _), _| *harness != id);
        self.skills.retain(|(harness, _), _| *harness != id);
        self.refreshing_commands
            .retain(|(harness, _)| *harness != id);
        self.refreshing_skills.retain(|(harness, _)| *harness != id);
    }
}

pub struct HarnessRegistry {
    pub(crate) installs: crate::rpc::Installations,
    slots: Mutex<HashMap<HarnessId, Slot>>,
    order: Mutex<Vec<HarnessId>>,
    /// This device's enabled set; `None` inner value = the default set.
    prefs: Mutex<HarnessPrefsFile>,
    /// Where the prefs persist; `None` (tests, bare registries) skips writes.
    prefs_path: Mutex<Option<PathBuf>>,
    /// Fair per-harness execution gates. Runs and title generation hold a
    /// shared lease; an accepted update queues an exclusive lease. Tokio's
    /// write-preferring FIFO policy prevents a stream of new runs from
    /// starving an update that is already waiting.
    gates: Mutex<HashMap<HarnessId, Arc<tokio::sync::RwLock<()>>>>,
    /// Shared so a catalog refresh can wait out an update without borrowing
    /// the registry (the menu returns as soon as a cached entry is in hand).
    pending_updates: Arc<Mutex<HashSet<HarnessId>>>,
    update_generation: tokio::sync::watch::Sender<u64>,
    catalogs: Arc<Mutex<CatalogCache>>,
}

async fn execution_lease_from(
    id: HarnessId,
    gate: Arc<tokio::sync::RwLock<()>>,
    pending_updates: Arc<Mutex<HashSet<HarnessId>>>,
    update_generation: tokio::sync::watch::Sender<u64>,
) -> tokio::sync::OwnedRwLockReadGuard<()> {
    let mut updates = update_generation.subscribe();
    loop {
        // The marker closes the small begin-update → writer-future polling
        // gap. Watch retains a generation change, so an update finishing
        // between this check and `changed()` cannot lose the wakeup.
        if pending_updates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&id)
        {
            let _ = updates.changed().await;
            continue;
        }
        let lease = Arc::clone(&gate).read_owned().await;
        if !pending_updates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&id)
        {
            return lease;
        }
        drop(lease);
    }
}

fn record_command_refresh(
    catalogs: &Mutex<CatalogCache>,
    id: HarnessId,
    cwd: PathBuf,
    generation: u64,
    result: Result<Vec<zeron_proto::SlashCommand>, HarnessError>,
) {
    let mut cache = catalogs.lock().unwrap_or_else(PoisonError::into_inner);
    if cache.generation_of(id) != generation {
        return;
    }
    cache.refreshing_commands.remove(&(id, cwd.clone()));
    if let Ok(commands) = result {
        cache.commands.insert((id, cwd), commands);
    }
}

fn record_skill_refresh(
    catalogs: &Mutex<CatalogCache>,
    id: HarnessId,
    cwd: PathBuf,
    generation: u64,
    result: Result<Option<Vec<zeron_proto::invocation::Skill>>, HarnessError>,
) {
    let mut cache = catalogs.lock().unwrap_or_else(PoisonError::into_inner);
    if cache.generation_of(id) != generation {
        return;
    }
    cache.refreshing_skills.remove(&(id, cwd.clone()));
    if let Ok(skills) = result {
        cache.skills.insert((id, cwd), skills);
    }
}

impl Default for HarnessRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HarnessRegistry {
    pub async fn discover_models(
        &self,
        id: HarnessId,
    ) -> Result<Vec<zeron_proto::Model>, HarnessError> {
        let lease = Arc::new(self.execution_lease(id).await);
        self.discover_models_with_lease(id, lease).await
    }

    /// A caller such as titling already holds a lease. Reacquiring after an
    /// update queues would deadlock it against its own existing reader.
    pub(crate) async fn discover_models_with_lease(
        &self,
        id: HarnessId,
        lease: Arc<tokio::sync::OwnedRwLockReadGuard<()>>,
    ) -> Result<Vec<zeron_proto::Model>, HarnessError> {
        let harness = self.resolve(id)?;
        tokio::spawn(async move {
            // An RPC cancellation must not drop the gate before the probe's
            // own deadline and child cleanup finish.
            let _lease = lease;
            harness.models().await
        })
        .await
        .map_err(|error| HarnessError::Protocol(format!("model discovery task failed: {error}")))?
    }

    /// Slash commands for `cwd`. A cached entry for this harness and directory
    /// returns immediately while a background refresh replaces it on success.
    /// Plugin, skill, and account changes show up one open later (the refresh
    /// triggered by this open); that's the trade for an instant menu. Errors
    /// are never cached.
    pub async fn discover_commands(
        &self,
        id: HarnessId,
        cwd: &Path,
    ) -> Result<Vec<zeron_proto::SlashCommand>, HarnessError> {
        let cwd = cwd.to_owned();
        if let Some(cached) = self.cached_commands(id, &cwd) {
            self.refresh_commands(id, cwd);
            return Ok(cached);
        }
        let lease = self.execution_lease(id).await;
        // After the lease, so a probe that only waited out an update still
        // caches. Before resolve, so a slot replaced underneath this call
        // cannot be stored as the replacement's catalog.
        let generation = self.catalog_generation(id);
        let harness = self.resolve(id)?;
        let store_cwd = cwd.clone();
        let discovered = tokio::spawn(async move {
            let _lease = lease;
            harness.commands_for(&cwd).await
        })
        .await
        .map_err(|error| {
            HarnessError::Protocol(format!("command discovery task failed: {error}"))
        })?;
        if let Ok(commands) = &discovered {
            self.store_commands(id, store_cwd, commands.clone(), generation);
        }
        discovered
    }

    /// Project skills for `cwd`. A cached entry for this harness and directory
    /// returns immediately while a background refresh replaces it on success.
    /// Plugin, skill, and account changes show up one open later (the refresh
    /// triggered by this open); that's the trade for an instant menu. Errors
    /// are never cached. `Ok(None)` is a real answer (this provider does not
    /// advertise skills) and is cached.
    pub async fn discover_skills(
        &self,
        id: HarnessId,
        cwd: &Path,
    ) -> Result<Option<Vec<zeron_proto::invocation::Skill>>, HarnessError> {
        let cwd = cwd.to_owned();
        if let Some(cached) = self.cached_skills(id, &cwd) {
            self.refresh_skills(id, cwd);
            return Ok(cached);
        }
        let lease = self.execution_lease(id).await;
        let generation = self.catalog_generation(id);
        let harness = self.resolve(id)?;
        let store_cwd = cwd.clone();
        let discovered = tokio::spawn(async move {
            // Retain the read lease through the adapter's deadline and cleanup,
            // even when the requesting RPC is dropped.
            let _lease = lease;
            harness.skills(&cwd).await
        })
        .await
        .map_err(|error| HarnessError::Protocol(format!("skill discovery task failed: {error}")))?;
        if let Ok(skills) = &discovered {
            self.store_skills(id, store_cwd, skills.clone(), generation);
        }
        discovered
    }

    fn catalog_generation(&self, id: HarnessId) -> u64 {
        self.catalogs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .generation_of(id)
    }

    fn cached_commands(&self, id: HarnessId, cwd: &Path) -> Option<Vec<zeron_proto::SlashCommand>> {
        self.catalogs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .commands
            .get(&(id, cwd.to_path_buf()))
            .cloned()
    }

    fn cached_skills(
        &self,
        id: HarnessId,
        cwd: &Path,
    ) -> Option<Option<Vec<zeron_proto::invocation::Skill>>> {
        self.catalogs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .skills
            .get(&(id, cwd.to_path_buf()))
            .cloned()
    }

    fn store_commands(
        &self,
        id: HarnessId,
        cwd: PathBuf,
        commands: Vec<zeron_proto::SlashCommand>,
        generation: u64,
    ) {
        let mut cache = self.catalogs.lock().unwrap_or_else(PoisonError::into_inner);
        if cache.generation_of(id) == generation {
            cache.commands.insert((id, cwd), commands);
        }
    }

    fn store_skills(
        &self,
        id: HarnessId,
        cwd: PathBuf,
        skills: Option<Vec<zeron_proto::invocation::Skill>>,
        generation: u64,
    ) {
        let mut cache = self.catalogs.lock().unwrap_or_else(PoisonError::into_inner);
        if cache.generation_of(id) == generation {
            cache.skills.insert((id, cwd), skills);
        }
    }

    fn invalidate_catalogs(&self, id: HarnessId) {
        self.catalogs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .invalidate(id);
    }

    /// Start one background reread of a cached command list. A refresh already
    /// in flight for this key is left alone. The lease is acquired inside the
    /// task so this returns without waiting out an update.
    fn refresh_commands(&self, id: HarnessId, cwd: PathBuf) {
        // Sample before resolve: a replacement between resolve and the
        // in-flight insert must not publish the old harness's list.
        let generation = self.catalog_generation(id);
        let Ok(harness) = self.resolve(id) else {
            return;
        };
        {
            let mut cache = self.catalogs.lock().unwrap_or_else(PoisonError::into_inner);
            if cache.generation_of(id) != generation
                || !cache.refreshing_commands.insert((id, cwd.clone()))
            {
                return;
            }
        }
        let catalogs = Arc::clone(&self.catalogs);
        let pending = Arc::clone(&self.pending_updates);
        let updates = self.update_generation.clone();
        let gate = self.gate(id);
        tokio::spawn(async move {
            let lease = execution_lease_from(id, gate, pending, updates).await;
            let result = harness.commands_for(&cwd).await;
            drop(lease);
            record_command_refresh(&catalogs, id, cwd, generation, result);
        });
    }

    fn refresh_skills(&self, id: HarnessId, cwd: PathBuf) {
        let generation = self.catalog_generation(id);
        let Ok(harness) = self.resolve(id) else {
            return;
        };
        {
            let mut cache = self.catalogs.lock().unwrap_or_else(PoisonError::into_inner);
            if cache.generation_of(id) != generation
                || !cache.refreshing_skills.insert((id, cwd.clone()))
            {
                return;
            }
        }
        let catalogs = Arc::clone(&self.catalogs);
        let pending = Arc::clone(&self.pending_updates);
        let updates = self.update_generation.clone();
        let gate = self.gate(id);
        tokio::spawn(async move {
            let lease = execution_lease_from(id, gate, pending, updates).await;
            let result = harness.skills(&cwd).await;
            drop(lease);
            record_skill_refresh(&catalogs, id, cwd, generation, result);
        });
    }

    pub fn new() -> Self {
        let (update_generation, _) = tokio::sync::watch::channel(0);
        Self {
            installs: Default::default(),
            slots: Mutex::new(HashMap::new()),
            order: Mutex::new(Vec::new()),
            prefs: Mutex::new(HarnessPrefsFile::default()),
            prefs_path: Mutex::new(None),
            gates: Mutex::new(HashMap::new()),
            pending_updates: Arc::new(Mutex::new(HashSet::new())),
            update_generation,
            catalogs: Arc::new(Mutex::new(CatalogCache {
                commands: HashMap::new(),
                skills: HashMap::new(),
                refreshing_commands: HashSet::new(),
                refreshing_skills: HashSet::new(),
                generation: HashMap::new(),
            })),
        }
    }

    fn gate(&self, id: HarnessId) -> Arc<tokio::sync::RwLock<()>> {
        self.gates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(id)
            .or_insert_with(|| Arc::new(tokio::sync::RwLock::new(())))
            .clone()
    }

    /// Shared lease held for the full lifetime of a harness subprocess.
    pub async fn execution_lease(&self, id: HarnessId) -> tokio::sync::OwnedRwLockReadGuard<()> {
        execution_lease_from(
            id,
            self.gate(id),
            Arc::clone(&self.pending_updates),
            self.update_generation.clone(),
        )
        .await
    }

    /// Exclusive lease held from immediately before update mutation through
    /// post-install verification.
    pub async fn update_lease(&self, id: HarnessId) -> tokio::sync::OwnedRwLockWriteGuard<()> {
        self.gate(id).write_owned().await
    }

    /// Mark an accepted update before queueing its writer. Existing persistent
    /// runtimes use this signal to retire at their next turn boundary instead
    /// of parking indefinitely while the writer waits.
    pub fn begin_update(&self, id: HarnessId) {
        self.pending_updates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id);
    }

    pub fn end_update(&self, id: HarnessId) {
        self.pending_updates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        // Drop catalogs before waking lease waiters, so a probe that was
        // only blocked on this update stores against the new generation
        // instead of racing a wipe.
        self.invalidate_catalogs(id);
        self.update_generation
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }

    pub fn update_pending(&self, id: HarnessId) -> bool {
        self.pending_updates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&id)
    }

    /// Run a synchronous dispatch-boundary action only if no update has been
    /// accepted for this harness. Holding the marker lock through the action
    /// gives direct steering a strict order against `begin_update`: either the
    /// prompt is accepted first and belongs to the existing run, or it waits
    /// behind the update through the ordinary dispatch path.
    pub(crate) fn while_update_clear<T>(
        &self,
        id: HarnessId,
        action: impl FnOnce() -> T,
    ) -> Option<T> {
        let pending = self
            .pending_updates
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if pending.contains(&id) {
            None
        } else {
            Some(action())
        }
    }

    fn slots(&self) -> MutexGuard<'_, HashMap<HarnessId, Slot>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn order(&self) -> MutexGuard<'_, Vec<HarnessId>> {
        self.order.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn prefs(&self) -> MutexGuard<'_, HarnessPrefsFile> {
        self.prefs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Load `harness-prefs.json` from the engine data dir and remember the
    /// path for writes. Corrupt/missing files fall back to the default set.
    pub fn load_prefs(&self, data_dir: &Path) {
        let path = data_dir.join("harness-prefs.json");
        let loaded = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<HarnessPrefsFile>(&text).ok())
            .unwrap_or_default();
        *self.prefs() = loaded;
        *self
            .prefs_path
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(path);
        self.migrate_legacy_prefs();
    }

    /// Fold a legacy allow-list into the opt-out shape: a registered harness
    /// missing from it was a deliberate "no", so it stays off. Rewrites the
    /// file once, which is what lets later installs auto-enable.
    fn migrate_legacy_prefs(&self) {
        let legacy = { self.prefs().enabled.take() };
        let Some(legacy) = legacy else { return };
        let registered: Vec<HarnessId> = self.order().iter().copied().collect();
        let disabled: Vec<HarnessId> = registered
            .into_iter()
            .filter(|id| auto_enabled(*id) && !legacy.contains(id))
            .collect();
        self.prefs().disabled = disabled;
        self.persist_prefs();
    }

    /// What this device offers: every harness whose CLI is FOUND, minus the
    /// user's explicit opt-outs. Enablement follows detection, so installing
    /// an agent is all it takes for it to appear in the composer.
    pub fn enabled_set(&self) -> Vec<HarnessId> {
        // Both guards drop before the installed probes run: `descriptors()`
        // takes `slots` then `order`, so holding `order` across a probe (which
        // takes `slots`) would invert the lock order.
        let registered: Vec<HarnessId> = self.order().iter().copied().collect();
        let disabled = self.prefs().disabled.clone();
        registered
            .into_iter()
            .filter(|id| auto_enabled(*id) && !disabled.contains(id) && self.installed_for(*id))
            .collect()
    }

    /// Whether this device's CLI probe passes for `id` (no spawn, no resolve).
    fn installed_for(&self, id: HarnessId) -> bool {
        match self.slots().get(&id) {
            Some(Slot::Ready(harness)) => harness.installed(),
            Some(Slot::Lazy { installed, .. }) => installed(),
            None => false,
        }
    }

    /// Flip one harness's enablement and persist. Refuses unknown harnesses,
    /// enabling one whose CLI is missing (the settings gate, enforced where
    /// the state lives), and disabling the last enabled harness — under
    /// detection-based enablement everything enabled is runnable, so the
    /// last one standing is always worth protecting (the composer needs
    /// something to run). A harness whose CLI is missing is never enabled
    /// in the first place, so turning it off is a clean no-op.
    pub fn set_enabled(&self, id: HarnessId, on: bool) -> Result<(), String> {
        if !self.slots().contains_key(&id) {
            return Err(format!("unknown harness {id:?}"));
        }
        if on && !auto_enabled(id) {
            return Err(format!("{id:?} cannot be enabled from Settings"));
        }
        if on && !self.installed_for(id) {
            return Err(format!("{id:?} CLI is not installed on this device"));
        }
        let enabled = self.enabled_set();
        match (on, enabled.contains(&id)) {
            (true, false) => {
                let mut prefs = self.prefs();
                prefs.disabled.retain(|h| *h != id);
            }
            (false, true) => {
                if enabled.len() == 1 {
                    return Err("cannot disable the last enabled harness".into());
                }
                let mut prefs = self.prefs();
                prefs.disabled.push(id);
            }
            _ => return Ok(()),
        }
        self.invalidate_catalogs(id);
        self.persist_prefs();
        Ok(())
    }

    /// Best-effort atomic write (temp + rename, the ui-settings pattern).
    fn persist_prefs(&self) {
        let Some(path) = self
            .prefs_path
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        else {
            return;
        };
        let json = match serde_json::to_string_pretty(&*self.prefs()) {
            Ok(json) => json,
            Err(err) => {
                tracing::warn!(error = %err, "harness-prefs serialize failed");
                return;
            }
        };
        let tmp = path.with_extension("json.tmp");
        if let Err(err) = std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, &path)) {
            tracing::warn!(error = %err, "harness-prefs save failed");
        }
    }

    pub fn title_settings(&self) -> TitleSettings {
        self.prefs().titles.clone()
    }

    pub fn set_title_settings(&self, mut settings: TitleSettings) -> Result<(), String> {
        if let Some(id) = settings.harness {
            if !zeron_harness::supports_titles(id) || !self.enabled_set().contains(&id) {
                return Err("Choose an enabled harness that supports title generation".into());
            }
        } else if settings.model.is_some() {
            return Err("Choose a title harness before choosing a model".into());
        }
        settings.model = settings.model.filter(|model| !model.trim().is_empty());
        self.prefs().titles = settings;
        self.persist_prefs();
        Ok(())
    }

    pub fn register(&self, harness: Arc<dyn Harness>) {
        let id = harness.id();
        let replaced = self.slots().insert(id, Slot::Ready(harness)).is_some();
        if replaced {
            // A cache keyed only by harness id must not survive the slot
            // being swapped for a different adapter.
            self.invalidate_catalogs(id);
        } else {
            self.order().push(id);
        }
    }

    /// Register a slot resolved on first `resolve` (the factory result is
    /// cached). `installed` is the CLI-presence probe run per `descriptors()`
    /// call; it must never spawn.
    pub fn register_lazy(
        &self,
        descriptor: HarnessDescriptor,
        installed: InstalledProbe,
        factory: Factory,
    ) {
        let id = descriptor.id;
        let replaced = self
            .slots()
            .insert(
                id,
                Slot::Lazy {
                    descriptor,
                    installed,
                    factory,
                },
            )
            .is_some();
        if replaced {
            self.invalidate_catalogs(id);
        } else {
            self.order().push(id);
        }
    }

    pub fn resolve(&self, id: HarnessId) -> Result<Arc<dyn Harness>, HarnessError> {
        let mut slots = self.slots();
        match slots.get(&id) {
            Some(Slot::Ready(harness)) => Ok(harness.clone()),
            Some(Slot::Lazy { factory, .. }) => {
                let harness = factory()?;
                slots.insert(id, Slot::Ready(harness.clone()));
                Ok(harness)
            }
            None => Err(HarnessError::NotInstalled(format!("{id:?}"))),
        }
    }

    /// Catalog for `ListHarnesses` — never forces a lazy resolve.
    pub fn descriptors(&self) -> Vec<HarnessDescriptor> {
        let enabled = self.enabled_set();
        let slots = self.slots();
        self.order()
            .iter()
            .filter_map(|id| {
                let mut descriptor = match slots.get(id) {
                    Some(Slot::Ready(harness)) => describe(harness.as_ref()),
                    Some(Slot::Lazy {
                        descriptor,
                        installed,
                        ..
                    }) => HarnessDescriptor {
                        installed: installed(),
                        ..descriptor.clone()
                    },
                    None => return None,
                };
                descriptor.enabled = Some(enabled.contains(id));
                descriptor.can_install = zeron_harness::install::can_install(*id);
                Some(descriptor)
            })
            .collect()
    }
}

/// The production registry: MockHarness (hidden from production pickers) plus a lazy
/// `claude-code` slot resolved through `zeron_harness` on first use (subprocess
/// discovery only happens when a run/model call actually needs it).
pub fn default_registry() -> HarnessRegistry {
    // Warm the login-shell PATH snapshot in the background so the first
    // claude/codex resolve doesn't pay the shell-startup latency inline.
    zeron_harness::shell_env::prewarm();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(MockHarness {
        script: vec![
            AgentEvent::TextDelta {
                text: "## Streaming pipeline\n\nEvery turn flows through the same path:\n\n".into(),
            },
            AgentEvent::TextDelta {
                text: "1. **Doc command** — the composer queues a durable `run` entry\n2. **Host executor** — the chat's host device marks it processed, then dispatches\n3. **Fold** — events fold into parts and diff into the Loro doc every 120ms\n\n".into(),
            },
            AgentEvent::ToolCall {
                id: "mock-tool-1".into(),
                call: zeron_proto::ToolCall::Exec {
                    command: "cargo test --workspace".into(),
                },
            },
            AgentEvent::ToolResult {
                id: "mock-tool-1".into(),
                is_error: false,
                output: None,
                diff: None,
            },
            AgentEvent::ToolCall {
                id: "mock-tool-2".into(),
                call: zeron_proto::ToolCall::Exec {
                    command: "git log -5 --oneline --decorate && git merge-base HEAD origin/main"
                        .into(),
                },
            },
            AgentEvent::ToolResult {
                id: "mock-tool-2".into(),
                is_error: false,
                output: None,
                diff: None,
            },
            AgentEvent::TextDelta {
                text: "The `SegmentWriter` appends into `LoroText` so the oplog stays RLE-merged:\n\n```rust\nfolded = fold_event_into_parts(&folded, &event);\nwriter.sync(&folded)?; // 120ms coalesced commits\n```\n\nSynced to every device through the session room. *Mock harness reporting in.*".into(),
            },
            AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            },
        ],
    }));
    registry.register_lazy(
        HarnessDescriptor {
            id: HarnessId::ClaudeCode,
            name: "Claude Code".into(),
            supports_steering: true,
            steering_mode: SteeringMode::StepBoundary,
            // Must mirror ClaudeHarness exactly — the descriptor-stability
            // rule (see the codex test below).
            reasoning_levels: vec![
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::XHigh,
                ReasoningLevel::Max,
            ],
            installed: true,
            can_install: false,
            enabled: None,
        },
        Box::new(|| zeron_harness::ClaudeHarness::new().installed()),
        Box::new(|| Ok(Arc::new(zeron_harness::ClaudeHarness::new()) as Arc<dyn Harness>)),
    );
    // Codex, same lazy pattern: the static descriptor mirrors AcpHarness::codex()
    // exactly (`describe()` after the first resolve must not change the
    // catalog entry) — "Codex" per the original HARNESS_LABEL, StepBoundary
    // steering via native `turn/steer`, and the unified reasoning ladder from
    // zeron_harness::codex::catalog. CLI discovery only happens when a
    // run/model call actually resolves the slot.
    registry.register_lazy(
        HarnessDescriptor {
            id: HarnessId::Codex,
            name: "Codex".into(),
            supports_steering: true,
            steering_mode: SteeringMode::StepBoundary,
            reasoning_levels: vec![
                ReasoningLevel::Minimal,
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::XHigh,
                ReasoningLevel::Max,
                ReasoningLevel::Ultra,
            ],
            installed: true,
            can_install: false,
            enabled: None,
        },
        Box::new(|| zeron_harness::CodexHarness::new().installed()),
        Box::new(|| Ok(Arc::new(zeron_harness::CodexHarness::new()) as Arc<dyn Harness>)),
    );
    // Cursor via the pinned @cursor/sdk shim (NOT ACP — that surface strips
    // subagent transcripts), same lazy pattern: the static descriptor mirrors
    // CursorHarness exactly. Native step-boundary steering; no effort ladder.
    registry.register_lazy(
        HarnessDescriptor {
            id: HarnessId::Cursor,
            name: "Cursor".into(),
            supports_steering: true,
            steering_mode: SteeringMode::StepBoundary,
            reasoning_levels: Vec::new(),
            installed: true,
            can_install: false,
            enabled: None,
        },
        Box::new(|| zeron_harness::CursorHarness::new().installed()),
        Box::new(|| Ok(Arc::new(zeron_harness::CursorHarness::new()) as Arc<dyn Harness>)),
    );
    // Devin over ACP (`devin acp`), same lazy pattern: the static descriptor
    // mirrors AcpHarness::devin() exactly. No steering extension (turn
    // boundaries) and no effort ladder — Devin bakes effort into the
    // advertised model ids instead of a `thought_level` option.
    registry.register_lazy(
        HarnessDescriptor {
            id: HarnessId::Devin,
            name: "Devin".into(),
            supports_steering: true,
            steering_mode: SteeringMode::TurnBoundary,
            reasoning_levels: Vec::new(),
            installed: true,
            can_install: false,
            enabled: None,
        },
        Box::new(|| zeron_harness::AcpHarness::devin().installed()),
        Box::new(|| Ok(Arc::new(zeron_harness::AcpHarness::devin()) as Arc<dyn Harness>)),
    );
    // Grok Build over ACP, same lazy pattern: the static descriptor mirrors
    // AcpHarness::grok() exactly. No `_session/steering` extension yet, so
    // steers deliver at turn boundaries; the effort ladder applies per
    // session via the `thought_level` config option.
    registry.register_lazy(
        HarnessDescriptor {
            id: HarnessId::Grok,
            name: "Grok".into(),
            supports_steering: true,
            steering_mode: SteeringMode::TurnBoundary,
            reasoning_levels: vec![
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
            ],
            installed: true,
            can_install: false,
            enabled: None,
        },
        Box::new(|| zeron_harness::AcpHarness::grok().installed()),
        Box::new(|| Ok(Arc::new(zeron_harness::AcpHarness::grok()) as Arc<dyn Harness>)),
    );
    // Hermes Agent over ACP (`hermes acp`), same lazy pattern: the static
    // descriptor mirrors AcpHarness::hermes() exactly. No steering extension
    // (turn boundaries) and no effort ladder — Hermes exposes no effort
    // config over ACP today (hybrid reasoning is model-internal).
    registry.register_lazy(
        HarnessDescriptor {
            id: HarnessId::Hermes,
            name: "Hermes".into(),
            supports_steering: true,
            steering_mode: SteeringMode::TurnBoundary,
            reasoning_levels: Vec::new(),
            installed: true,
            can_install: false,
            enabled: None,
        },
        Box::new(|| zeron_harness::AcpHarness::hermes().installed()),
        Box::new(|| Ok(Arc::new(zeron_harness::AcpHarness::hermes()) as Arc<dyn Harness>)),
    );
    // Native Pi RPC. Thinking levels are discovered per model.
    registry.register_lazy(
        HarnessDescriptor {
            id: HarnessId::Pi,
            name: "Pi".into(),
            supports_steering: true,
            steering_mode: SteeringMode::StepBoundary,
            reasoning_levels: Vec::new(),
            installed: true,
            can_install: false,
            enabled: None,
        },
        Box::new(|| zeron_harness::PiHarness::new().installed()),
        Box::new(|| Ok(Arc::new(zeron_harness::PiHarness::new()) as Arc<dyn Harness>)),
    );
    // opencode over its NATIVE HTTP/SSE protocol (the one the opencode
    // desktop app speaks — `opencode serve` + the /global/event bus), same
    // lazy pattern: the static descriptor mirrors OpencodeHarness exactly.
    // Turn-boundary steering; the effort ladder rides model VARIANTS (the
    // run sends the first advertised variant id for the picked level).
    registry.register_lazy(
        HarnessDescriptor {
            id: HarnessId::Opencode,
            name: "OpenCode".into(),
            supports_steering: true,
            steering_mode: SteeringMode::TurnBoundary,
            reasoning_levels: vec![
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::XHigh,
                ReasoningLevel::Max,
            ],
            installed: true,
            can_install: false,
            enabled: None,
        },
        Box::new(|| zeron_harness::OpencodeHarness::new().installed()),
        Box::new(|| Ok(Arc::new(zeron_harness::OpencodeHarness::new()) as Arc<dyn Harness>)),
    );
    // antigravity over acp (google's agy_acp_server), same lazy pattern: the
    // static descriptor mirrors AcpHarness::antigravity() exactly. No steering
    // extension (turn boundaries), and effort is baked into the model ids, so
    // the ladder lives on each model rather than the harness.
    registry.register_lazy(
        HarnessDescriptor {
            id: HarnessId::Antigravity,
            name: "Antigravity".into(),
            supports_steering: true,
            steering_mode: SteeringMode::TurnBoundary,
            reasoning_levels: Vec::new(),
            installed: true,
            can_install: false,
            enabled: None,
        },
        Box::new(|| zeron_harness::AcpHarness::antigravity().installed()),
        Box::new(|| Ok(Arc::new(zeron_harness::AcpHarness::antigravity()) as Arc<dyn Harness>)),
    );
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mid_turn_steering_requires_support_and_a_step_boundary() {
        let mut descriptor = HarnessDescriptor {
            id: HarnessId::Mock,
            name: "Mock".into(),
            supports_steering: true,
            steering_mode: SteeringMode::StepBoundary,
            reasoning_levels: Vec::new(),
            installed: true,
            can_install: false,
            enabled: Some(true),
        };
        assert!(descriptor.steers_mid_turn());

        descriptor.steering_mode = SteeringMode::TurnBoundary;
        assert!(!descriptor.steers_mid_turn());

        descriptor.steering_mode = SteeringMode::StepBoundary;
        descriptor.supports_steering = false;
        assert!(!descriptor.steers_mid_turn());
    }

    #[test]
    fn lazy_slot_lists_without_resolving() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let registry = HarnessRegistry::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        registry.register_lazy(
            HarnessDescriptor {
                id: HarnessId::Mock,
                name: "Lazy Mock".into(),
                supports_steering: true,
                steering_mode: SteeringMode::StepBoundary,
                reasoning_levels: vec![],
                installed: true,
                can_install: false,
                enabled: None,
            },
            Box::new(|| false),
            Box::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                Err(HarnessError::NotInstalled("nope".into()))
            }),
        );
        let listed = registry.descriptors();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Lazy Mock");
        // The listing runs the probe, not the stored placeholder.
        assert!(!listed[0].installed);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "listing must not force a resolve"
        );
        assert!(registry.resolve(HarnessId::Mock).is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn default_registry_lists_mock_claude_codex_and_grok_slots() {
        let registry = default_registry();
        let ids: Vec<HarnessId> = registry.descriptors().iter().map(|d| d.id).collect();
        assert_eq!(
            ids,
            vec![
                HarnessId::Mock,
                HarnessId::ClaudeCode,
                HarnessId::Codex,
                HarnessId::Cursor,
                HarnessId::Devin,
                HarnessId::Grok,
                HarnessId::Hermes,
                HarnessId::Pi,
                HarnessId::Opencode,
                HarnessId::Antigravity
            ]
        );
        assert!(registry.resolve(HarnessId::Mock).is_ok());
        assert!(registry.resolve(HarnessId::ClaudeCode).is_ok());
        // A codex-configured chat resolves the right harness (construction is
        // cheap; CLI discovery is deferred to models()/run()).
        let codex = registry.resolve(HarnessId::Codex).unwrap();
        assert_eq!(codex.id(), HarnessId::Codex);
        // Grok resolves through the shared ACP harness; its descriptor must
        // mirror the resolved harness (descriptor-stability rule).
        let grok = registry.resolve(HarnessId::Grok).unwrap();
        assert_eq!(grok.id(), HarnessId::Grok);
        assert_eq!(grok.display_name(), "Grok");
        assert_eq!(grok.steering_mode(), SteeringMode::StepBoundary);
        assert_eq!(
            grok.reasoning_levels(),
            &[
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High
            ]
        );
        // Cursor, Devin, Hermes and Pi mirror their specs the same way.
        let cursor = registry.resolve(HarnessId::Cursor).unwrap();
        assert_eq!(cursor.id(), HarnessId::Cursor);
        assert_eq!(cursor.display_name(), "Cursor");
        assert_eq!(cursor.steering_mode(), SteeringMode::StepBoundary);
        assert!(cursor.reasoning_levels().is_empty());
        let devin = registry.resolve(HarnessId::Devin).unwrap();
        assert_eq!(devin.id(), HarnessId::Devin);
        assert_eq!(devin.display_name(), "Devin");
        assert_eq!(devin.steering_mode(), SteeringMode::StepBoundary);
        assert!(devin.reasoning_levels().is_empty());
        let hermes = registry.resolve(HarnessId::Hermes).unwrap();
        assert_eq!(hermes.id(), HarnessId::Hermes);
        assert_eq!(hermes.display_name(), "Hermes");
        assert_eq!(hermes.steering_mode(), SteeringMode::TurnBoundary);
        assert!(hermes.reasoning_levels().is_empty());
        let opencode = registry.resolve(HarnessId::Opencode).unwrap();
        assert_eq!(opencode.id(), HarnessId::Opencode);
        assert_eq!(opencode.display_name(), "OpenCode");
        assert_eq!(opencode.steering_mode(), SteeringMode::StepBoundary);
        assert_eq!(
            opencode.reasoning_levels(),
            &[
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::XHigh,
                ReasoningLevel::Max,
            ]
        );
        let antigravity = registry.resolve(HarnessId::Antigravity).unwrap();
        assert_eq!(antigravity.id(), HarnessId::Antigravity);
        assert_eq!(antigravity.display_name(), "Antigravity");
        assert_eq!(antigravity.steering_mode(), SteeringMode::TurnBoundary);
        assert!(antigravity.reasoning_levels().is_empty());
        let pi = registry.resolve(HarnessId::Pi).unwrap();
        assert_eq!(pi.id(), HarnessId::Pi);
        assert_eq!(pi.display_name(), "Pi");
        assert_eq!(pi.steering_mode(), SteeringMode::StepBoundary);
        assert!(pi.reasoning_levels().is_empty());
    }

    /// Catalogs serialized by engines that predate the `installed`/`enabled`
    /// fields must keep deserializing — installed, and enabled per the
    /// detection fallback.
    #[test]
    fn descriptor_without_new_fields_parses_with_fallbacks() {
        let parse = |id: &str| -> HarnessDescriptor {
            serde_json::from_str(&format!(
                r#"{{
                    "id": "{id}",
                    "name": "x",
                    "supportsSteering": true,
                    "steeringMode": "step-boundary",
                    "reasoningLevels": []
                }}"#
            ))
            .unwrap()
        };
        let claude = parse("claude-code");
        assert!(claude.installed);
        assert!(!claude.can_install);
        assert_eq!(claude.enabled, None);
        // Unknown enablement follows detection: a found CLI is offered...
        assert!(descriptor_enabled(&claude));
        // ...and one this device never found is not.
        let missing = HarnessDescriptor {
            installed: false,
            can_install: false,
            ..parse("grok")
        };
        assert!(!descriptor_enabled(&missing));
    }

    /// A registry slot for the tests below: installed probe fixed, factory
    /// never expected to run.
    fn test_slot(registry: &HarnessRegistry, id: HarnessId, installed: bool) {
        registry.register_lazy(
            HarnessDescriptor {
                id,
                name: format!("{id:?}"),
                supports_steering: true,
                steering_mode: SteeringMode::StepBoundary,
                reasoning_levels: vec![],
                installed: true,
                can_install: false,
                enabled: None,
            },
            Box::new(move || installed),
            Box::new(|| Err(HarnessError::NotInstalled("test slot".into()))),
        );
    }

    /// `descriptors()` stamps the per-device enabled flag; `set_enabled`
    /// guards the gate (no enabling missing CLIs, no disabling the last one)
    /// and persists across a reload.
    #[test]
    fn enablement_stamps_guards_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let registry = HarnessRegistry::new();
        registry.load_prefs(dir.path());
        test_slot(&registry, HarnessId::ClaudeCode, true);
        test_slot(&registry, HarnessId::Codex, true);
        test_slot(&registry, HarnessId::Grok, true);
        test_slot(&registry, HarnessId::Hermes, false);

        // Enablement follows detection: the three found CLIs are on with no
        // prefs file at all, and the missing one is off.
        let flags: Vec<(HarnessId, Option<bool>)> = registry
            .descriptors()
            .into_iter()
            .map(|d| (d.id, d.enabled))
            .collect();
        assert_eq!(
            flags,
            vec![
                (HarnessId::ClaudeCode, Some(true)),
                (HarnessId::Codex, Some(true)),
                (HarnessId::Grok, Some(true)),
                (HarnessId::Hermes, Some(false)),
            ]
        );

        // The gate: a missing CLI can't be enabled; unknown ids refuse.
        assert!(registry.set_enabled(HarnessId::Hermes, true).is_err());
        assert!(registry.set_enabled(HarnessId::Pi, true).is_err());
        assert!(registry.set_enabled(HarnessId::Mock, true).is_err());
        // Installed CLIs toggle both ways; no-op flips are fine.
        registry.set_enabled(HarnessId::Grok, true).unwrap();
        registry.set_enabled(HarnessId::Grok, true).unwrap();
        registry.set_enabled(HarnessId::Codex, false).unwrap();
        registry.set_enabled(HarnessId::ClaudeCode, false).unwrap();
        // Grok is the last one standing — refusing keeps the composer usable.
        assert!(registry.set_enabled(HarnessId::Grok, false).is_err());
        assert_eq!(registry.enabled_set(), vec![HarnessId::Grok]);

        // A fresh registry over the same data dir reads the persisted opt-outs.
        let reloaded = HarnessRegistry::new();
        reloaded.load_prefs(dir.path());
        test_slot(&reloaded, HarnessId::ClaudeCode, true);
        test_slot(&reloaded, HarnessId::Codex, true);
        test_slot(&reloaded, HarnessId::Grok, true);
        assert_eq!(reloaded.enabled_set(), vec![HarnessId::Grok]);
    }

    /// The point of following detection: an agent installed after the user has
    /// already edited Settings turns itself on, while the ones they switched
    /// off stay off. An allow-list can't express that — it can't tell "the
    /// user said no" from "this wasn't installed yet".
    #[test]
    fn newly_found_harnesses_enable_themselves_without_reviving_opt_outs() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let registry = HarnessRegistry::new();
        registry.load_prefs(dir.path());
        test_slot(&registry, HarnessId::ClaudeCode, true);
        test_slot(&registry, HarnessId::Codex, true);
        // Not installed yet — the probe flips when the user installs the CLI.
        let found = Arc::new(AtomicBool::new(false));
        let probe = Arc::clone(&found);
        registry.register_lazy(
            HarnessDescriptor {
                id: HarnessId::Grok,
                name: "Grok".into(),
                supports_steering: true,
                steering_mode: SteeringMode::TurnBoundary,
                reasoning_levels: vec![],
                installed: true,
                can_install: false,
                enabled: None,
            },
            Box::new(move || probe.load(Ordering::SeqCst)),
            Box::new(|| Err(HarnessError::NotInstalled("test slot".into()))),
        );
        registry.set_enabled(HarnessId::Codex, false).unwrap();
        assert_eq!(registry.enabled_set(), vec![HarnessId::ClaudeCode]);

        // The CLI appears mid-session; no restart, no visit to Settings.
        found.store(true, Ordering::SeqCst);
        assert_eq!(
            registry.enabled_set(),
            vec![HarnessId::ClaudeCode, HarnessId::Grok]
        );
        // The opt-out survives the reload that picks the new agent up.
        let reloaded = HarnessRegistry::new();
        reloaded.load_prefs(dir.path());
        test_slot(&reloaded, HarnessId::ClaudeCode, true);
        test_slot(&reloaded, HarnessId::Codex, true);
        test_slot(&reloaded, HarnessId::Grok, true);
        assert_eq!(
            reloaded.enabled_set(),
            vec![HarnessId::ClaudeCode, HarnessId::Grok]
        );
    }

    #[test]
    fn antigravity_detection_ignores_legacy_opt_in_and_preserves_opt_out() {
        let dir = tempfile::tempdir().unwrap();
        let registry = HarnessRegistry::new();
        registry.load_prefs(dir.path());
        test_slot(&registry, HarnessId::ClaudeCode, true);
        test_slot(&registry, HarnessId::Antigravity, true);
        let prefs: HarnessPrefsFile =
            serde_json::from_str(r#"{"optedIn":["antigravity"],"disabled":["antigravity"]}"#)
                .unwrap();
        *registry.prefs() = prefs;
        assert_eq!(registry.enabled_set(), vec![HarnessId::ClaudeCode]);

        registry.set_enabled(HarnessId::Antigravity, true).unwrap();
        let both = vec![HarnessId::ClaudeCode, HarnessId::Antigravity];
        assert_eq!(registry.enabled_set(), both);

        let reloaded = HarnessRegistry::new();
        reloaded.load_prefs(dir.path());
        test_slot(&reloaded, HarnessId::ClaudeCode, true);
        test_slot(&reloaded, HarnessId::Antigravity, true);
        assert_eq!(reloaded.enabled_set(), both);

        reloaded.set_enabled(HarnessId::Antigravity, false).unwrap();
        assert_eq!(reloaded.enabled_set(), vec![HarnessId::ClaudeCode]);
    }

    #[cfg(unix)]
    #[test]
    fn antigravity_detection_subprocess() {
        let Ok(expected) = std::env::var("ZERON_TEST_AGY_INSTALLED") else {
            return;
        };
        let registry = HarnessRegistry::new();
        let data = tempfile::tempdir().unwrap();
        registry.load_prefs(data.path());
        registry.register(Arc::new(zeron_harness::AcpHarness::antigravity()));
        let expected = expected == "true";
        assert_eq!(registry.descriptors()[0].installed, expected);
        assert_eq!(
            registry.enabled_set().contains(&HarnessId::Antigravity),
            expected
        );
        assert_eq!(descriptor_enabled(&registry.descriptors()[0]), expected);
    }

    #[cfg(unix)]
    #[test]
    fn antigravity_server_path_controls_detection_and_enablement() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let cli = bin.join("agy");
        std::fs::write(&cli, "#!/bin/sh\nexit 91\n").unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        let server = bin.join("agy_acp_server.par");
        for installed in [false, true] {
            if installed {
                std::fs::write(&server, "#!/bin/sh\nexit 91\n").unwrap();
                std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "registry::tests::antigravity_detection_subprocess",
                    "--nocapture",
                ])
                .env("HOME", home.path())
                .env("PATH", &bin)
                .env_remove("ANTIGRAVITY_ACP_EXECUTABLE")
                .env("ZERON_NO_LOGIN_SHELL", "1")
                .env("ZERON_TEST_AGY_INSTALLED", installed.to_string())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }

    /// The mock resolves on every machine, so detection alone would enable it
    /// and let the last REAL agent be switched off — leaving a composer with
    /// nothing runnable behind a guard that thinks it is covered.
    #[test]
    fn detection_never_enables_the_mock() {
        let registry = default_registry();
        let enabled = registry.enabled_set();
        assert!(!enabled.contains(&HarnessId::Mock), "{enabled:?}");
        let mock = registry
            .descriptors()
            .into_iter()
            .find(|d| d.id == HarnessId::Mock)
            .expect("mock slot");
        assert!(mock.installed);
        assert_eq!(mock.enabled, Some(false));
    }

    /// A prefs file from the fixed-default era is an allow-list: everything
    /// registered and absent from it was a deliberate "no", so it converts to
    /// opt-outs rather than silently re-enabling on the next launch.
    #[test]
    fn legacy_allow_list_migrates_to_opt_outs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("harness-prefs.json"),
            r#"{ "enabled": ["claude-code"] }"#,
        )
        .unwrap();
        let registry = HarnessRegistry::new();
        test_slot(&registry, HarnessId::ClaudeCode, true);
        test_slot(&registry, HarnessId::Codex, true);
        test_slot(&registry, HarnessId::Grok, true);
        registry.load_prefs(dir.path());
        assert_eq!(registry.enabled_set(), vec![HarnessId::ClaudeCode]);

        // The rewritten file is the new shape, and the legacy key is gone.
        let text = std::fs::read_to_string(dir.path().join("harness-prefs.json")).unwrap();
        assert!(!text.contains("enabled"), "{text}");
        assert!(text.contains("codex") && text.contains("grok"), "{text}");
        // An agent registered after the migration is new, not a past "no".
        test_slot(&registry, HarnessId::Cursor, true);
        assert_eq!(
            registry.enabled_set(),
            vec![HarnessId::ClaudeCode, HarnessId::Cursor]
        );
    }

    /// The fresh-machine shape (#128): no CLIs installed at all. Under
    /// detection-based enablement nothing is enabled to begin with — no
    /// dimmed default toggles to dismiss — and switching an uninstalled
    /// harness "off" is a clean no-op, not an error.
    #[test]
    fn machine_without_clis_enables_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let registry = HarnessRegistry::new();
        registry.load_prefs(dir.path());
        test_slot(&registry, HarnessId::ClaudeCode, false);
        test_slot(&registry, HarnessId::Codex, false);

        assert_eq!(registry.enabled_set(), Vec::<HarnessId>::new());
        registry.set_enabled(HarnessId::Codex, false).unwrap();
        registry.set_enabled(HarnessId::ClaudeCode, false).unwrap();
        assert_eq!(registry.enabled_set(), Vec::<HarnessId>::new());

        let reloaded = HarnessRegistry::new();
        reloaded.load_prefs(dir.path());
        assert_eq!(reloaded.enabled_set(), Vec::<HarnessId>::new());
    }

    /// The Codex lazy descriptor must be indistinguishable from `describe()`
    /// after the first resolve — otherwise the catalog entry silently changes
    /// the moment the harness is used (name/ladder flip in the picker rail).
    /// (KNOWN GAP, predates this slot: the claude-code descriptor advertises
    /// `[Ultrathink]` while the resolved adapter reports `[Low..Max]` — left
    /// as-is here; flagged for its own pass.)
    #[test]
    fn codex_lazy_descriptor_matches_resolved_harness() {
        let registry = default_registry();
        let before = registry
            .descriptors()
            .into_iter()
            .find(|d| d.id == HarnessId::Codex)
            .unwrap();
        registry.resolve(HarnessId::Codex).unwrap();
        let after = registry
            .descriptors()
            .into_iter()
            .find(|d| d.id == HarnessId::Codex)
            .unwrap();
        assert_eq!(before.name, after.name);
        assert_eq!(before.supports_steering, after.supports_steering);
        assert_eq!(before.steering_mode, after.steering_mode);
        assert_eq!(before.reasoning_levels, after.reasoning_levels);
    }
}

#[cfg(test)]
mod title_tests {
    use super::*;

    #[test]
    fn title_preferences_persist_and_validate_harness_model_pairs() {
        let dir = tempfile::tempdir().unwrap();
        let registry = HarnessRegistry::new();
        registry.load_prefs(dir.path());
        registry.register(Arc::new(
            zeron_harness::ClaudeHarness::new().with_executable(std::env::current_exe().unwrap()),
        ));
        let settings = TitleSettings {
            harness: Some(HarnessId::ClaudeCode),
            model: Some("haiku-test".into()),
        };
        registry.set_title_settings(settings.clone()).unwrap();
        let reloaded = HarnessRegistry::new();
        reloaded.load_prefs(dir.path());
        assert_eq!(reloaded.title_settings(), settings);
        assert!(
            registry
                .set_title_settings(TitleSettings {
                    harness: None,
                    model: Some("orphan".into())
                })
                .is_err()
        );
        assert!(
            registry
                .set_title_settings(TitleSettings {
                    harness: Some(HarnessId::Cursor),
                    model: None
                })
                .is_err()
        );
        assert_eq!(registry.title_settings(), settings);
        registry
            .set_title_settings(TitleSettings::default())
            .unwrap();
        reloaded.load_prefs(dir.path());
        assert_eq!(reloaded.title_settings(), TitleSettings::default());
    }

    #[test]
    fn old_harness_preferences_default_to_automatic_titles() {
        let prefs: HarnessPrefsFile = serde_json::from_str(r#"{"disabled":["codex"]}"#).unwrap();
        assert_eq!(prefs.titles, TitleSettings::default());
        assert_eq!(prefs.disabled, vec![HarnessId::Codex]);
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    struct DiscoveryHarness {
        started: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }
    #[async_trait::async_trait]
    impl Harness for DiscoveryHarness {
        fn id(&self) -> HarnessId {
            HarnessId::Codex
        }
        fn display_name(&self) -> &str {
            "Discovery fixture"
        }
        fn supports_steering(&self) -> bool {
            false
        }
        fn steering_mode(&self) -> SteeringMode {
            SteeringMode::TurnBoundary
        }
        fn reasoning_levels(&self) -> &[ReasoningLevel] {
            &[]
        }
        async fn models(&self) -> Result<Vec<zeron_proto::Model>, HarnessError> {
            self.started.notify_one();
            self.release.notified().await;
            Ok(vec![])
        }
        async fn commands(&self) -> Result<Vec<zeron_proto::SlashCommand>, HarnessError> {
            self.started.notify_one();
            self.release.notified().await;
            Ok(vec![])
        }
        async fn run(
            &self,
            _: zeron_proto::RunRequest,
            _: zeron_harness::RunControls,
        ) -> Result<
            futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>,
            HarnessError,
        > {
            unreachable!("discovery must not start a conversation")
        }
    }

    #[tokio::test]
    async fn discovery_waits_for_updates_and_keeps_lease_after_caller_cancellation() {
        use std::time::Duration;
        for commands in [false, true] {
            let registry = Arc::new(HarnessRegistry::new());
            let harness = Arc::new(DiscoveryHarness {
                started: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            });
            registry.register(harness.clone());
            registry.begin_update(HarnessId::Codex);
            let installing = registry.update_lease(HarnessId::Codex).await;
            let caller = tokio::spawn({
                let registry = registry.clone();
                async move {
                    if commands {
                        registry
                            .discover_commands(HarnessId::Codex, Path::new("/tmp"))
                            .await
                            .map(|_| ())
                    } else {
                        registry.discover_models(HarnessId::Codex).await.map(|_| ())
                    }
                }
            });
            assert!(
                tokio::time::timeout(Duration::from_millis(30), harness.started.notified())
                    .await
                    .is_err()
            );
            drop(installing);
            registry.end_update(HarnessId::Codex);
            tokio::time::timeout(Duration::from_secs(1), harness.started.notified())
                .await
                .unwrap();
            caller.abort();
            assert!(caller.await.unwrap_err().is_cancelled());
            registry.begin_update(HarnessId::Codex);
            let mut writer = tokio::spawn({
                let registry = registry.clone();
                async move { registry.update_lease(HarnessId::Codex).await }
            });
            assert!(
                tokio::time::timeout(Duration::from_millis(30), &mut writer)
                    .await
                    .is_err()
            );
            harness.release.notify_one();
            drop(
                tokio::time::timeout(Duration::from_secs(1), writer)
                    .await
                    .unwrap()
                    .unwrap(),
            );
            registry.end_update(HarnessId::Codex);
        }
    }

    #[tokio::test]
    async fn queued_update_writer_precedes_later_dispatches() {
        let registry = Arc::new(HarnessRegistry::new());
        let running = registry.execution_lease(HarnessId::Codex).await;
        registry.begin_update(HarnessId::Codex);
        assert!(registry.update_pending(HarnessId::Codex));

        let writer_registry = registry.clone();
        let (writer_acquired_tx, writer_acquired_rx) = tokio::sync::oneshot::channel();
        let (release_writer_tx, release_writer_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _writer = writer_registry.update_lease(HarnessId::Codex).await;
            let _ = writer_acquired_tx.send(());
            let _ = release_writer_rx.await;
        });
        tokio::task::yield_now().await;

        let reader_registry = registry.clone();
        let (reader_acquired_tx, reader_acquired_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _reader = reader_registry.execution_lease(HarnessId::Codex).await;
            let _ = reader_acquired_tx.send(());
        });

        drop(running);
        writer_acquired_rx.await.unwrap();
        let mut reader_acquired_rx = Box::pin(reader_acquired_rx);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(20),
                &mut reader_acquired_rx
            )
            .await
            .is_err(),
            "later reader jumped ahead of the queued update writer"
        );
        let _ = release_writer_tx.send(());
        registry.end_update(HarnessId::Codex);
        reader_acquired_rx.await.unwrap();
        assert!(!registry.update_pending(HarnessId::Codex));
    }

    #[test]
    fn update_marker_rejects_new_boundary_actions() {
        let registry = HarnessRegistry::new();
        assert_eq!(
            registry.while_update_clear(HarnessId::Codex, || 42),
            Some(42)
        );
        registry.begin_update(HarnessId::Codex);
        let mut called = false;
        assert_eq!(
            registry.while_update_clear(HarnessId::Codex, || called = true),
            None
        );
        assert!(!called);
        registry.end_update(HarnessId::Codex);
    }
}

#[cfg(test)]
mod catalog_cache_tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    struct SlowCatalog {
        id: HarnessId,
        hold: Mutex<Option<Arc<tokio::sync::Notify>>>,
        commands: Mutex<Result<Vec<zeron_proto::SlashCommand>, String>>,
        skills: Mutex<Result<Option<Vec<zeron_proto::invocation::Skill>>, String>>,
        hits: AtomicUsize,
        waiting: AtomicUsize,
        finished: AtomicUsize,
        /// When set, a probe returns the value it observed on entry. Tests use
        /// this so an in-flight refresh keeps the list it started with after
        /// the live value moves on.
        freeze: AtomicBool,
    }

    impl SlowCatalog {
        fn new(id: HarnessId) -> Self {
            Self {
                id,
                hold: Mutex::new(None),
                commands: Mutex::new(Ok(Vec::new())),
                skills: Mutex::new(Ok(None)),
                hits: AtomicUsize::new(0),
                waiting: AtomicUsize::new(0),
                finished: AtomicUsize::new(0),
                freeze: AtomicBool::new(false),
            }
        }

        fn set_commands(&self, commands: Result<Vec<zeron_proto::SlashCommand>, String>) {
            *self.commands.lock().unwrap() = commands;
        }

        fn set_skills(&self, skills: Result<Option<Vec<zeron_proto::invocation::Skill>>, String>) {
            *self.skills.lock().unwrap() = skills;
        }

        fn arm(&self) -> Arc<tokio::sync::Notify> {
            let notify = Arc::new(tokio::sync::Notify::new());
            *self.hold.lock().unwrap() = Some(Arc::clone(&notify));
            notify
        }

        fn disarm(&self) {
            *self.hold.lock().unwrap() = None;
        }

        async fn wait_if_held(&self) {
            let hold = self.hold.lock().unwrap().clone();
            if let Some(hold) = hold {
                let notified = hold.notified();
                tokio::pin!(notified);
                // Register before publishing `waiting`, so a test that observes
                // the counter can wake this call without losing the permit.
                if !notified.as_mut().enable() {
                    self.waiting.fetch_add(1, Ordering::SeqCst);
                    notified.await;
                    self.waiting.fetch_sub(1, Ordering::SeqCst);
                }
            }
        }

        async fn probe<T: Clone>(
            &self,
            value: &Mutex<Result<T, String>>,
        ) -> Result<T, HarnessError> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            let captured = value.lock().unwrap().clone();
            self.wait_if_held().await;
            let result = if self.freeze.load(Ordering::SeqCst) {
                captured
            } else {
                value.lock().unwrap().clone()
            };
            self.finished.fetch_add(1, Ordering::SeqCst);
            result.map_err(HarnessError::Protocol)
        }
    }

    #[async_trait::async_trait]
    impl Harness for SlowCatalog {
        fn id(&self) -> HarnessId {
            self.id
        }
        fn display_name(&self) -> &str {
            "Slow catalog"
        }
        fn supports_steering(&self) -> bool {
            false
        }
        fn steering_mode(&self) -> SteeringMode {
            SteeringMode::TurnBoundary
        }
        fn reasoning_levels(&self) -> &[ReasoningLevel] {
            &[]
        }
        async fn models(&self) -> Result<Vec<zeron_proto::Model>, HarnessError> {
            Ok(Vec::new())
        }
        async fn commands_for(
            &self,
            _: &Path,
        ) -> Result<Vec<zeron_proto::SlashCommand>, HarnessError> {
            self.probe(&self.commands).await
        }
        async fn skills(
            &self,
            _: &Path,
        ) -> Result<Option<Vec<zeron_proto::invocation::Skill>>, HarnessError> {
            self.probe(&self.skills).await
        }
        async fn run(
            &self,
            _: zeron_proto::RunRequest,
            _: zeron_harness::RunControls,
        ) -> Result<
            futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>,
            HarnessError,
        > {
            unreachable!("catalog discovery must not start a conversation")
        }
    }

    fn command(name: &str) -> zeron_proto::SlashCommand {
        zeron_proto::SlashCommand {
            name: name.into(),
            description: String::new(),
            input_hint: None,
        }
    }

    fn skill(name: &str) -> zeron_proto::invocation::Skill {
        zeron_proto::invocation::Skill {
            name: name.into(),
            path: format!("/skills/{name}"),
            description: String::new(),
            enabled: true,
            command: None,
        }
    }

    fn names(commands: &[zeron_proto::SlashCommand]) -> Vec<&str> {
        commands
            .iter()
            .map(|command| command.name.as_str())
            .collect()
    }

    async fn wait_until(mut ready: impl FnMut() -> bool) {
        let start = tokio::time::Instant::now();
        while !ready() {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "timed out waiting for the catalog probe"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn commands_now(
        registry: &HarnessRegistry,
        id: HarnessId,
        cwd: &Path,
    ) -> Vec<zeron_proto::SlashCommand> {
        tokio::time::timeout(Duration::from_secs(2), registry.discover_commands(id, cwd))
            .await
            .expect("cached commands should return without waiting for discovery")
            .unwrap()
    }

    async fn skills_now(
        registry: &HarnessRegistry,
        id: HarnessId,
        cwd: &Path,
    ) -> Option<Vec<zeron_proto::invocation::Skill>> {
        tokio::time::timeout(Duration::from_secs(2), registry.discover_skills(id, cwd))
            .await
            .expect("cached skills should return without waiting for discovery")
            .unwrap()
    }

    #[tokio::test]
    async fn cached_commands_return_immediately_and_refresh_in_background() {
        let registry = HarnessRegistry::new();
        let catalog = Arc::new(SlowCatalog::new(HarnessId::Codex));
        registry.register(Arc::clone(&catalog) as Arc<dyn Harness>);
        catalog.set_commands(Ok(vec![command("review")]));
        let cwd = Path::new("/work");

        assert_eq!(
            names(&commands_now(&registry, HarnessId::Codex, cwd).await),
            ["review"]
        );

        catalog.set_commands(Ok(vec![command("later")]));
        let hold = catalog.arm();
        assert_eq!(
            names(&commands_now(&registry, HarnessId::Codex, cwd).await),
            ["review"]
        );
        wait_until(|| catalog.waiting.load(Ordering::SeqCst) >= 1).await;
        // The open above already has a refresh in flight. Another open must
        // not start a second one.
        assert_eq!(
            names(&commands_now(&registry, HarnessId::Codex, cwd).await),
            ["review"]
        );
        let hits = catalog.hits.load(Ordering::SeqCst);
        let started = tokio::time::Instant::now();
        while started.elapsed() < Duration::from_secs(1) {
            assert_eq!(
                catalog.hits.load(Ordering::SeqCst),
                hits,
                "refresh was not coalesced"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        hold.notify_waiters();
        wait_until(|| catalog.finished.load(Ordering::SeqCst) >= 2).await;
        catalog.disarm();
        assert_eq!(
            names(&commands_now(&registry, HarnessId::Codex, cwd).await),
            ["later"]
        );
    }

    #[tokio::test]
    async fn command_cache_is_keyed_by_directory() {
        let registry = Arc::new(HarnessRegistry::new());
        let catalog = Arc::new(SlowCatalog::new(HarnessId::Codex));
        registry.register(Arc::clone(&catalog) as Arc<dyn Harness>);
        catalog.set_commands(Ok(vec![command("review")]));
        let first = Path::new("/work/a");
        let second = Path::new("/work/b");
        assert_eq!(
            names(&commands_now(&registry, HarnessId::Codex, first).await),
            ["review"]
        );

        catalog.set_commands(Ok(vec![command("other")]));
        let hold = catalog.arm();
        let pending = tokio::spawn({
            let registry = Arc::clone(&registry);
            async move { registry.discover_commands(HarnessId::Codex, second).await }
        });
        wait_until(|| catalog.waiting.load(Ordering::SeqCst) >= 1).await;
        assert!(
            !pending.is_finished(),
            "a different directory must not reuse the cached command list"
        );
        hold.notify_waiters();
        let discovered = pending.await.unwrap().unwrap();
        assert_eq!(names(&discovered), ["other"]);

        catalog.disarm();
        assert_eq!(
            names(&commands_now(&registry, HarnessId::Codex, first).await),
            ["review"]
        );
    }

    #[tokio::test]
    async fn cached_skills_return_immediately_and_refresh_in_background() {
        let registry = HarnessRegistry::new();
        let catalog = Arc::new(SlowCatalog::new(HarnessId::Codex));
        registry.register(Arc::clone(&catalog) as Arc<dyn Harness>);
        catalog.set_skills(Ok(None));
        let cwd = Path::new("/work");

        assert!(skills_now(&registry, HarnessId::Codex, cwd).await.is_none());

        catalog.set_skills(Ok(Some(vec![skill("ship")])));
        let hold = catalog.arm();
        assert!(
            skills_now(&registry, HarnessId::Codex, cwd).await.is_none(),
            "a provider with no skills is a cached answer"
        );
        wait_until(|| catalog.waiting.load(Ordering::SeqCst) >= 1).await;
        hold.notify_waiters();
        wait_until(|| catalog.finished.load(Ordering::SeqCst) >= 2).await;
        catalog.disarm();

        let refreshed = skills_now(&registry, HarnessId::Codex, cwd).await.unwrap();
        assert_eq!(refreshed.len(), 1);
        assert_eq!(refreshed[0].name, "ship");
    }

    #[tokio::test]
    async fn catalog_errors_are_not_cached() {
        let registry = HarnessRegistry::new();
        let catalog = Arc::new(SlowCatalog::new(HarnessId::Codex));
        registry.register(Arc::clone(&catalog) as Arc<dyn Harness>);
        let cwd = Path::new("/work");

        catalog.set_commands(Err("down".into()));
        let error = registry
            .discover_commands(HarnessId::Codex, cwd)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("down"), "{error}");

        catalog.set_commands(Ok(vec![command("review")]));
        let hold = catalog.arm();
        let registry = Arc::new(registry);
        let pending = tokio::spawn({
            let registry = Arc::clone(&registry);
            async move { registry.discover_commands(HarnessId::Codex, cwd).await }
        });
        wait_until(|| catalog.waiting.load(Ordering::SeqCst) >= 1).await;
        assert!(
            !pending.is_finished(),
            "a failed discovery must not be served from the cache"
        );
        hold.notify_waiters();
        assert_eq!(names(&pending.await.unwrap().unwrap()), ["review"]);

        catalog.disarm();
        catalog.set_commands(Err("refresh failed".into()));
        let hold = catalog.arm();
        assert_eq!(
            names(&commands_now(&registry, HarnessId::Codex, cwd).await),
            ["review"]
        );
        wait_until(|| catalog.waiting.load(Ordering::SeqCst) >= 1).await;
        hold.notify_waiters();
        wait_until(|| catalog.finished.load(Ordering::SeqCst) >= 3).await;
        catalog.disarm();
        catalog.set_commands(Ok(vec![command("fresh")]));
        assert_eq!(
            names(&commands_now(&registry, HarnessId::Codex, cwd).await),
            ["review"],
            "a failed refresh must keep the previous command list"
        );
    }

    #[tokio::test]
    async fn end_update_clears_catalog_cache() {
        let registry = Arc::new(HarnessRegistry::new());
        let catalog = Arc::new(SlowCatalog::new(HarnessId::Codex));
        registry.register(Arc::clone(&catalog) as Arc<dyn Harness>);
        catalog.set_commands(Ok(vec![command("review")]));
        let cwd = Path::new("/work");
        assert_eq!(
            names(&commands_now(&registry, HarnessId::Codex, cwd).await),
            ["review"]
        );

        registry.end_update(HarnessId::Codex);
        catalog.set_commands(Ok(vec![command("fresh")]));
        let hold = catalog.arm();
        let pending = tokio::spawn({
            let registry = Arc::clone(&registry);
            async move { registry.discover_commands(HarnessId::Codex, cwd).await }
        });
        wait_until(|| catalog.waiting.load(Ordering::SeqCst) >= 1).await;
        assert!(
            !pending.is_finished(),
            "an update must drop the cached command list"
        );
        hold.notify_waiters();
        assert_eq!(names(&pending.await.unwrap().unwrap()), ["fresh"]);
    }

    #[tokio::test]
    async fn disabling_a_harness_clears_catalog_cache() {
        let registry = Arc::new(HarnessRegistry::new());
        let catalog = Arc::new(SlowCatalog::new(HarnessId::Codex));
        registry.register(Arc::clone(&catalog) as Arc<dyn Harness>);
        registry.register(Arc::new(SlowCatalog::new(HarnessId::ClaudeCode)) as Arc<dyn Harness>);
        catalog.set_commands(Ok(vec![command("review")]));
        let cwd = Path::new("/work");
        assert_eq!(
            names(&commands_now(&registry, HarnessId::Codex, cwd).await),
            ["review"]
        );

        registry.set_enabled(HarnessId::Codex, true).unwrap();
        let hold = catalog.arm();
        assert_eq!(
            names(&commands_now(&registry, HarnessId::Codex, cwd).await),
            ["review"]
        );
        wait_until(|| catalog.waiting.load(Ordering::SeqCst) >= 1).await;
        hold.notify_waiters();
        wait_until(|| catalog.finished.load(Ordering::SeqCst) >= 2).await;
        catalog.disarm();

        registry.set_enabled(HarnessId::Codex, false).unwrap();
        catalog.set_commands(Ok(vec![command("fresh")]));
        let hold = catalog.arm();
        let pending = tokio::spawn({
            let registry = Arc::clone(&registry);
            async move { registry.discover_commands(HarnessId::Codex, cwd).await }
        });
        wait_until(|| catalog.hits.load(Ordering::SeqCst) >= 3).await;
        assert!(
            !pending.is_finished(),
            "disabling a harness must drop its cached command list"
        );
        hold.notify_waiters();
        assert_eq!(names(&pending.await.unwrap().unwrap()), ["fresh"]);
    }

    #[tokio::test]
    async fn replacing_a_harness_slot_clears_catalog_cache() {
        let registry = Arc::new(HarnessRegistry::new());
        let cwd = Path::new("/work");
        let first = Arc::new(SlowCatalog::new(HarnessId::Codex));
        registry.register(Arc::clone(&first) as Arc<dyn Harness>);
        first.set_commands(Ok(vec![command("review")]));
        assert_eq!(
            names(&commands_now(&registry, HarnessId::Codex, cwd).await),
            ["review"]
        );

        let second = Arc::new(SlowCatalog::new(HarnessId::Codex));
        second.set_commands(Ok(vec![command("fresh")]));
        let hold = second.arm();
        registry.register(Arc::clone(&second) as Arc<dyn Harness>);
        let pending = tokio::spawn({
            let registry = Arc::clone(&registry);
            async move { registry.discover_commands(HarnessId::Codex, cwd).await }
        });
        wait_until(|| second.waiting.load(Ordering::SeqCst) >= 1).await;
        assert!(
            !pending.is_finished(),
            "replacing a harness must drop the previous catalog"
        );
        hold.notify_waiters();
        assert_eq!(names(&pending.await.unwrap().unwrap()), ["fresh"]);

        let third = Arc::new(SlowCatalog::new(HarnessId::Codex));
        third.set_commands(Ok(vec![command("lazy")]));
        let hold = third.arm();
        let factory_harness = Arc::clone(&third);
        registry.register_lazy(
            HarnessDescriptor {
                id: HarnessId::Codex,
                name: "Replacement".into(),
                supports_steering: false,
                steering_mode: SteeringMode::TurnBoundary,
                reasoning_levels: Vec::new(),
                installed: true,
                can_install: false,
                enabled: None,
            },
            Box::new(|| true),
            Box::new(move || Ok(Arc::clone(&factory_harness) as Arc<dyn Harness>)),
        );
        let pending = tokio::spawn({
            let registry = Arc::clone(&registry);
            async move { registry.discover_commands(HarnessId::Codex, cwd).await }
        });
        wait_until(|| third.waiting.load(Ordering::SeqCst) >= 1).await;
        assert!(
            !pending.is_finished(),
            "replacing a lazy slot must drop the previous catalog"
        );
        hold.notify_waiters();
        assert_eq!(names(&pending.await.unwrap().unwrap()), ["lazy"]);
    }

    #[tokio::test]
    async fn in_flight_refresh_cannot_republish_after_invalidation() {
        let registry = HarnessRegistry::new();
        let catalog = Arc::new(SlowCatalog::new(HarnessId::Codex));
        registry.register(Arc::clone(&catalog) as Arc<dyn Harness>);
        catalog.set_commands(Ok(vec![command("review")]));
        let cwd = Path::new("/work");
        assert_eq!(
            names(&commands_now(&registry, HarnessId::Codex, cwd).await),
            ["review"]
        );

        catalog.freeze.store(true, Ordering::SeqCst);
        catalog.set_commands(Ok(vec![command("stale-refresh")]));
        let hold = catalog.arm();
        assert_eq!(
            names(&commands_now(&registry, HarnessId::Codex, cwd).await),
            ["review"]
        );
        wait_until(|| catalog.waiting.load(Ordering::SeqCst) >= 1).await;

        registry.end_update(HarnessId::Codex);
        catalog.set_commands(Ok(vec![command("fresh")]));
        catalog.disarm();
        assert_eq!(
            names(&commands_now(&registry, HarnessId::Codex, cwd).await),
            ["fresh"]
        );

        hold.notify_waiters();
        wait_until(|| catalog.finished.load(Ordering::SeqCst) >= 3).await;
        assert_eq!(
            names(&commands_now(&registry, HarnessId::Codex, cwd).await),
            ["fresh"],
            "a refresh started before the update must not restore the old list"
        );
    }
}
