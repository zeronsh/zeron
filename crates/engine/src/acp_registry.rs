//! The ACP registry on this device (Settings → Providers → Add an agent):
//! the registry's agents as this device sees them, and adding, updating and
//! removing one. The registry's index is fetched here, never by an app, and
//! kept in `{data_dir}/acp-registry/` (with each agent's icon), so the
//! page shows the last copy at once and a device offline still lists it.
//! Installs and launches are [`zeron_harness::acp_registry`]'s; an added
//! agent is a harness like any other ([`HarnessRegistry::register_registry_agent`]).
//!
//!   ListAcpRegistry { refresh? }  → { agents: [Listing], fetchedAt?, offline }
//!   AddAcpAgent { id }            → { agent: Listing, harnesses }  (also updates)
//!   RemoveAcpAgent { id }         → { harnesses }
//!   GetAcpAgentEnv { id }         → { env }   (`KEY=VALUE` a line)
//!   SetAcpAgentEnv { id, env }    → { env }
//!   AddOwnAgent { name, command, env } → { agent, harnesses }
//!
//! All three are forwardable (`targetDeviceId`): an agent is added to the
//! device Providers shows.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zeron_harness::acp_registry::{self as reg, Entry, Readiness};
use zeron_proto::{HarnessId, RegistryAgent};
use zeron_rpc::{RpcError, RpcReply, methods};

use crate::registry::HarnessRegistry;

/// A fresh enough copy: the CDN's own max-age.
const FRESH: Duration = Duration::from_secs(5 * 60);
const ICON_FETCHES: usize = 8;

/// One agent as the page shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub id: String,
    /// Its harness id once added (`acp.<id>`), else the one it would get.
    pub harness: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub authors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    /// How it would install here: `binary`, `npx` or `uvx` (none: no build
    /// for this device).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<reg::Kind>,
    /// What this device lacks to run it.
    pub readiness: Readiness,
    /// The host a binary downloads from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// A binary checked against the registry's sha256 (true), one the
    /// registry publishes no checksum for (false); none for packages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<bool>,
    /// The built-in harness Zeron drives this agent with, if it does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub built_in: Option<String>,
    /// The version added on this device, if it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<String>,
    /// Being added (or updated) now.
    pub adding: bool,
    /// No longer in the registry (added before it left).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub delisted: bool,
    /// One of your own (`own.<slug>`), never the registry's.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub own: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    etag: Option<String>,
    #[serde(default)]
    fetched_at: Option<u64>,
}

#[derive(Default)]
struct Cache {
    entries: Vec<Entry>,
    icons: HashMap<String, String>,
    etag: Option<String>,
    fetched_at: Option<u64>,
    loaded: bool,
}

pub struct AcpRegistry {
    dir: PathBuf,
    cache: Mutex<Cache>,
    refreshing: tokio::sync::Mutex<()>,
    adding: Mutex<HashSet<String>>,
}

static SERVICE: OnceLock<Arc<AcpRegistry>> = OnceLock::new();

/// Sets the registry up for this engine: its added agents live under its
/// data dir, and each is registered as a harness. (Not on a Cloud device:
/// it runs only the agents it's provisioned with.)
pub fn start(data_dir: &Path, registry: &HarnessRegistry) {
    reg::set_agents_root(data_dir.join("agents"));
    let _ = SERVICE.set(Arc::new(AcpRegistry::new(data_dir.join("acp-registry"))));
    if crate::runner::is_cloud_platform() {
        return;
    }
    for record in reg::records() {
        registry.register_registry_agent(&record);
    }
}

/// Whether an added agent is the registry's (its updates the registry's), not
/// one of your own.
pub(crate) fn from_registry(agent: RegistryAgent) -> bool {
    reg::record(agent).is_some_and(|r| r.kind != reg::Kind::Own)
}

/// The registry's current version of an added agent: the update
/// coordinator's feed (`LatestSource::Registry`). The copy kept serves when
/// the registry can't be reached.
pub(crate) async fn latest_version(harness: HarnessId) -> Result<String, String> {
    let agent = harness.registry().ok_or("not an agent from the registry")?;
    let service = service().map_err(|e| e.to_string())?;
    let _ = service.refresh(false).await;
    let cache = service.cache();
    cache
        .entries
        .iter()
        .find(|e| e.id == agent.registry_id())
        .map(|e| e.version.clone())
        .ok_or_else(|| "it's no longer in the ACP registry".into())
}

/// Installs the registry's build of an added agent: the update
/// coordinator's install (`UpdatePlan::Registry`), under its exclusive
/// lease. `expected`: the version the check found, refused if the registry
/// has moved on since (check again first).
pub(crate) async fn install_latest(agent: RegistryAgent, expected: Option<&str>) -> Result<(), String> {
    let service = service().map_err(|e| e.to_string())?;
    let current = service.cache().entries.iter().find(|e| e.id == agent.registry_id()).map(|e| e.version.clone());
    if let (Some(expected), Some(current)) = (expected, current.as_deref())
        && expected != current
    {
        return Err("the registry has another version now; check again".into());
    }
    service.add(agent.registry_id()).await.map(drop).map_err(|e| e.to_string())
}

fn service() -> Result<Arc<AcpRegistry>, RpcError> {
    SERVICE
        .get()
        .cloned()
        .ok_or_else(|| RpcError::Failed("The agent registry isn't available here".into()))
}

pub(crate) fn handles(method: &str) -> bool {
    matches!(
        method,
        methods::LIST_ACP_REGISTRY
            | methods::ADD_ACP_AGENT
            | methods::REMOVE_ACP_AGENT
            | methods::GET_ACP_AGENT_ENV
            | methods::SET_ACP_AGENT_ENV
            | methods::GET_ACP_AGENT_SIGN_IN
            | methods::SIGN_OUT_ACP_AGENT
            | methods::SET_ACP_AGENT_KEY
            | methods::ADD_OWN_AGENT
    )
}

/// How long an agent's sign-in, as it last said it, stands.
const SIGN_IN_FRESH: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Each added agent's sign-in as it last said it (asking starts it).
fn sign_ins()
-> &'static Mutex<std::collections::HashMap<RegistryAgent, (std::time::Instant, zeron_harness::acp::RegistrySignIn)>> {
    static SIGN_INS: OnceLock<
        Mutex<std::collections::HashMap<RegistryAgent, (std::time::Instant, zeron_harness::acp::RegistrySignIn)>>,
    > = OnceLock::new();
    SIGN_INS.get_or_init(Default::default)
}

/// A sign-in or sign-out ran: what the agent said before no longer stands.
pub(crate) fn forget_sign_in(agent: RegistryAgent) {
    sign_ins().lock().unwrap_or_else(PoisonError::into_inner).remove(&agent);
}

fn harness_of(agent: RegistryAgent) -> Result<zeron_harness::AcpHarness, RpcError> {
    let record = reg::record(agent).ok_or_else(|| RpcError::Failed(format!("{} isn't added here", agent.registry_id())))?;
    Ok(zeron_harness::AcpHarness::registry(agent, &record.name))
}

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
struct EnvParams {
    id: String,
    #[serde(default)]
    env: String,
}

#[derive(Deserialize)]
struct KeyParams {
    id: String,
    var: String,
    #[serde(default)]
    value: String,
}

#[derive(Deserialize)]
struct SignInParams {
    id: String,
    #[serde(default)]
    refresh: bool,
}

#[derive(Deserialize)]
struct OwnParams {
    name: String,
    command: String,
    #[serde(default)]
    env: String,
}

fn agent_of(id: &str) -> Result<RegistryAgent, RpcError> {
    RegistryAgent::new(id).ok_or_else(|| RpcError::BadParams(format!("{id:?} isn't an agent id")))
}

#[derive(Deserialize, Default)]
struct ListParams {
    #[serde(default)]
    refresh: bool,
}

pub(crate) async fn dispatch(
    registry: &HarnessRegistry,
    updates: Option<&crate::harness_updates::HarnessUpdateCoordinator>,
    method: &str,
    params: Value,
) -> Result<RpcReply, RpcError> {
    if crate::runner::is_cloud_platform() {
        return Err(RpcError::Failed("Cloud runs only the agents it's provisioned with".into()));
    }
    let service = service()?;
    match method {
        methods::LIST_ACP_REGISTRY => {
            let p: ListParams = serde_json::from_value(params).unwrap_or_default();
            let offline = service.refresh(p.refresh).await.is_err();
            let cache = service.cache();
            RpcReply::value(&json!({
                "agents": service.listings(&cache),
                "fetchedAt": cache.fetched_at,
                "offline": offline,
                "platform": reg::platform(),
            }))
        }
        methods::ADD_ACP_AGENT => {
            let p: IdParams = parse(params)?;
            let agent = agent_of(&p.id)?;
            let harness = HarnessId::Registry(agent);
            // Added already: an update, through the update coordinator (its
            // lease waits out the agent's chats), the one updater there is.
            let record = match (reg::record(agent), updates) {
                (Some(_), Some(updates)) => {
                    updates.check_one(harness).await.map_err(RpcError::Failed)?;
                    updates.apply(harness).await.map_err(RpcError::Failed)?;
                    reg::record(agent).ok_or_else(|| RpcError::Failed("the update didn't land".into()))?
                }
                _ => service.add(&p.id).await?,
            };
            registry.register_registry_agent(&record);
            if let Some(updates) = updates {
                updates.track(harness);
            }
            let cache = service.cache();
            let agent = service.listings(&cache).into_iter().find(|l| l.id == p.id);
            RpcReply::value(&json!({ "agent": agent, "harnesses": registry.descriptors_for(true) }))
        }
        methods::REMOVE_ACP_AGENT => {
            let p: IdParams = parse(params)?;
            let agent = RegistryAgent::new(&p.id)
                .ok_or_else(|| RpcError::BadParams(format!("{:?} isn't a registry id", p.id)))?;
            registry.unregister(HarnessId::Registry(agent));
            if let Some(updates) = updates {
                updates.untrack(HarnessId::Registry(agent));
            }
            reg::remove(agent).map_err(|e| RpcError::Failed(e.to_string()))?;
            RpcReply::value(&json!({ "harnesses": registry.descriptors_for(true) }))
        }
        methods::GET_ACP_AGENT_ENV => {
            let p: EnvParams = parse(params)?;
            let env = reg::user_env(agent_of(&p.id)?);
            RpcReply::value(&json!({ "env": reg::env_text(&env) }))
        }
        methods::SET_ACP_AGENT_ENV => {
            let p: EnvParams = parse(params)?;
            let env = reg::set_user_env(agent_of(&p.id)?, &p.env).map_err(|e| RpcError::Failed(e.to_string()))?;
            RpcReply::value(&json!({ "env": reg::env_text(&env) }))
        }
        methods::GET_ACP_AGENT_SIGN_IN => {
            let p: SignInParams = parse(params)?;
            let agent = agent_of(&p.id)?;
            let kept = sign_ins().lock().unwrap_or_else(PoisonError::into_inner).get(&agent).cloned();
            if let Some((at, state)) = kept.filter(|(at, _)| !p.refresh && at.elapsed() < SIGN_IN_FRESH) {
                let _ = at;
                return RpcReply::value(&state);
            }
            let state = harness_of(agent)?
                .registry_sign_in_state()
                .await
                .map_err(|e| RpcError::Failed(e.to_string()))?;
            sign_ins()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(agent, (std::time::Instant::now(), state.clone()));
            RpcReply::value(&state)
        }
        methods::SIGN_OUT_ACP_AGENT => {
            let p: IdParams = parse(params)?;
            let agent = agent_of(&p.id)?;
            let outcome = harness_of(agent)?.sign_out_registry().await;
            forget_sign_in(agent);
            outcome.map_err(|e| RpcError::Failed(e.to_string()))?;
            tracing::info!(agent = agent.registry_id(), "registry agent signed out");
            RpcReply::value(&json!({ "ok": true }))
        }
        methods::SET_ACP_AGENT_KEY => {
            let p: KeyParams = parse(params)?;
            let agent = agent_of(&p.id)?;
            let valid = !p.var.is_empty()
                && p.var.len() <= 64
                && p.var.starts_with(|c: char| c.is_ascii_uppercase())
                && p.var.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            let value = p.value.trim();
            if !valid || value.chars().any(char::is_control) || value.len() > 4096 {
                return Err(RpcError::BadParams("not a key".into()));
            }
            let mut env = reg::user_env(agent);
            if value.is_empty() {
                env.remove(&p.var);
            } else {
                env.insert(p.var.clone(), value.to_owned());
            }
            reg::set_user_env(agent, &reg::env_text(&env)).map_err(|e| RpcError::Failed(e.to_string()))?;
            forget_sign_in(agent);
            tracing::info!(agent = agent.registry_id(), var = %p.var, set = !value.is_empty(), "registry agent key set");
            RpcReply::value(&json!({ "ok": true }))
        }
        methods::ADD_OWN_AGENT => {
            let p: OwnParams = parse(params)?;
            let record = reg::add_own(&p.name, &p.command, &p.env).map_err(|e| RpcError::Failed(e.to_string()))?;
            registry.register_registry_agent(&record);
            let cache = service.cache();
            let agent = service.listings(&cache).into_iter().find(|l| l.id == record.id);
            RpcReply::value(&json!({ "agent": agent, "harnesses": registry.descriptors_for(true) }))
        }
        other => Err(RpcError::UnknownMethod(other.to_owned())),
    }
}

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params).map_err(|e| RpcError::BadParams(e.to_string()))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

impl AcpRegistry {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            cache: Mutex::new(Cache::default()),
            refreshing: tokio::sync::Mutex::new(()),
            adding: Mutex::new(HashSet::new()),
        }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Cache> {
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if !cache.loaded {
            cache.loaded = true;
            self.load_saved(&mut cache);
        }
        cache
    }

    /// The copy kept on disk, read once.
    fn load_saved(&self, cache: &mut Cache) {
        if let Some(entries) = std::fs::read(self.dir.join("registry.json"))
            .ok()
            .and_then(|bytes| reg::parse_index(&bytes).ok())
        {
            cache.entries = entries;
        }
        let saved: Saved = std::fs::read(self.dir.join("state.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        cache.etag = saved.etag;
        cache.fetched_at = saved.fetched_at;
        if let Ok(icons) = std::fs::read_dir(self.dir.join("icons")) {
            for icon in icons.filter_map(Result::ok) {
                let name = icon.file_name().to_string_lossy().to_string();
                if let Some(id) = name.strip_suffix(".svg")
                    && let Some(svg) = std::fs::read_to_string(icon.path()).ok().and_then(|t| reg::clean_svg(&t))
                {
                    cache.icons.insert(id.to_owned(), svg);
                }
            }
        }
    }

    /// Fetches the index unless the copy is fresh (or `force`), then any
    /// icons not yet kept. Errs when the registry can't be reached: the
    /// copy kept still lists.
    async fn refresh(&self, force: bool) -> Result<(), String> {
        let _one = self.refreshing.lock().await;
        let (etag, fresh, empty) = {
            let cache = self.cache();
            let fresh = cache
                .fetched_at
                .is_some_and(|at| now().saturating_sub(at) < FRESH.as_secs());
            (cache.etag.clone(), fresh, cache.entries.is_empty())
        };
        if fresh && !force && !empty {
            return Ok(());
        }
        let url = reg::index_url();
        let (bytes, etag) = reg::fetch_index(&url, if empty { None } else { etag.as_deref() }).await?;
        if let Some(bytes) = &bytes {
            let entries = reg::parse_index(bytes)?;
            let _ = std::fs::create_dir_all(&self.dir);
            let tmp = self.dir.join(".registry.json.tmp");
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, self.dir.join("registry.json"));
            }
            self.cache().entries = entries;
        }
        {
            let mut cache = self.cache();
            cache.etag = etag.clone();
            cache.fetched_at = Some(now());
            let saved = Saved { etag, fetched_at: cache.fetched_at };
            let _ = std::fs::create_dir_all(&self.dir);
            let _ = std::fs::write(self.dir.join("state.json"), serde_json::to_vec(&saved).unwrap_or_default());
        }
        self.fetch_icons().await;
        Ok(())
    }

    async fn fetch_icons(&self) {
        use futures::StreamExt;
        let wanted: Vec<(String, String)> = {
            let cache = self.cache();
            cache
                .entries
                .iter()
                .filter(|e| !cache.icons.contains_key(&e.id))
                .filter_map(|e| Some((e.id.clone(), e.icon.clone()?)))
                .collect()
        };
        let fetched: Vec<(String, Option<String>)> = futures::stream::iter(wanted)
            .map(|(id, url)| async move { (id, reg::fetch_icon(&url).await) })
            .buffer_unordered(ICON_FETCHES)
            .collect()
            .await;
        let dir = self.dir.join("icons");
        let _ = std::fs::create_dir_all(&dir);
        let mut cache = self.cache();
        for (id, svg) in fetched {
            if let Some(svg) = svg {
                let _ = std::fs::write(dir.join(format!("{id}.svg")), &svg);
                cache.icons.insert(id, svg);
            }
        }
    }

    /// Every agent of the registry, then any added here that has since
    /// left it, in the registry's order (by name).
    fn listings(&self, cache: &Cache) -> Vec<Listing> {
        let records: BTreeMap<String, reg::Record> =
            reg::records().into_iter().map(|r| (r.id.clone(), r)).collect();
        let adding = self.adding.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let mut out: Vec<Listing> = cache
            .entries
            .iter()
            .map(|e| {
                let choice = reg::choose(e);
                let host = match &choice {
                    Some(reg::Choice::Binary(t)) => reqwest::Url::parse(&t.archive)
                        .ok()
                        .and_then(|u| u.host_str().map(str::to_owned)),
                    Some(reg::Choice::Npx(_)) => Some("npmjs.com".into()),
                    Some(reg::Choice::Uvx(_)) => Some("pypi.org".into()),
                    None => None,
                };
                Listing {
                    id: e.id.clone(),
                    harness: format!("{}{}", zeron_proto::REGISTRY_SLUG_PREFIX, e.id),
                    name: e.name.clone(),
                    version: e.version.clone(),
                    description: e.description.clone(),
                    authors: e.authors.clone(),
                    license: e.license.clone(),
                    repository: e.repository.clone(),
                    website: e.website.clone(),
                    kind: choice.as_ref().map(reg::Choice::kind),
                    readiness: reg::readiness(e),
                    host,
                    checksum: reg::checksum(e),
                    built_in: reg::built_in(&e.id).map(str::to_owned),
                    added: records.get(&e.id).map(|r| r.version.clone()),
                    adding: adding.contains(&e.id),
                    delisted: false,
                    own: false,
                    icon: cache.icons.get(&e.id).cloned(),
                }
            })
            .collect();
        for record in records.values() {
            if out.iter().any(|l| l.id == record.id) {
                continue;
            }
            out.push(Listing {
                id: record.id.clone(),
                harness: format!("{}{}", zeron_proto::REGISTRY_SLUG_PREFIX, record.id),
                name: record.name.clone(),
                version: record.version.clone(),
                description: record.description.clone(),
                authors: record.authors.clone(),
                license: None,
                repository: None,
                website: None,
                kind: Some(record.kind),
                readiness: Readiness::Ready,
                host: None,
                checksum: None,
                built_in: None,
                added: Some(record.version.clone()),
                adding: adding.contains(&record.id),
                delisted: record.kind != reg::Kind::Own,
                own: record.kind == reg::Kind::Own,
                icon: record.icon_svg.clone(),
            });
        }
        out.sort_by_key(|l| l.name.to_lowercase());
        out
    }

    /// Adds (or updates to the registry's version) one agent.
    async fn add(&self, id: &str) -> Result<reg::Record, RpcError> {
        if reg::built_in(id).is_some() {
            return Err(RpcError::Failed("Zeron drives this agent itself".into()));
        }
        // The registry's current entry (a fresh copy if this one is missing).
        let mut entry = self.cache().entries.iter().find(|e| e.id == id).cloned();
        if entry.is_none() {
            let _ = self.refresh(true).await;
            entry = self.cache().entries.iter().find(|e| e.id == id).cloned();
        }
        let entry = entry.ok_or_else(|| RpcError::Failed(format!("{id} isn't in the registry")))?;
        {
            let mut adding = self.adding.lock().unwrap_or_else(PoisonError::into_inner);
            if !adding.insert(id.to_owned()) {
                return Err(RpcError::Failed("already adding".into()));
            }
        }
        struct Done<'a>(&'a AcpRegistry, String);
        impl Drop for Done<'_> {
            fn drop(&mut self) {
                self.0.adding.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.1);
            }
        }
        let _done = Done(self, id.to_owned());
        let kept = self.cache().icons.get(id).cloned();
        let icon = match kept {
            Some(icon) => Some(icon),
            None => match &entry.icon {
                Some(url) => reg::fetch_icon(url).await,
                None => None,
            },
        };
        reg::install(&entry, icon).await.map_err(|e| RpcError::Failed(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_methods_are_handled_here() {
        assert!(handles(methods::LIST_ACP_REGISTRY));
        assert!(handles(methods::ADD_ACP_AGENT));
        assert!(handles(methods::REMOVE_ACP_AGENT));
        assert!(!handles(methods::LIST_HARNESSES));
    }
}
