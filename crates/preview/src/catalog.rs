//! Persisted names are independent of ephemeral process/listener observations.
use crate::discovery::{Listener, command_identity, framework};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicU32},
    time::SystemTime,
};
use tokio::sync::watch;
use zeron_proto::{PREVIEW_PROXY_PORT, PreviewService, PreviewSnapshot};

#[derive(Clone)]
pub struct Catalog(Arc<Inner>);
struct Inner {
    file: PathBuf,
    device_id: String,
    device_name: String,
    state: Mutex<State>,
    changes: watch::Sender<PreviewSnapshot>,
}
struct State {
    names: Names,
    routes: HashMap<String, LocalRoute>,
    remote: HashMap<String, Vec<PreviewService>>,
    proxy_port: u16,
    error: Option<String>,
    /// Project and service labels. Built once at load and updated on insert
    /// and prune, so a scan does not walk the registry to rebuild it.
    used_labels: HashSet<String>,
}
#[derive(Clone)]
pub struct LocalRoute {
    pub service: PreviewService,
    pub listener: Listener,
}
#[derive(Default, Serialize, Deserialize)]
struct Names {
    device_label: String,
    #[serde(default)]
    peer_labels: BTreeMap<String, String>,
    projects: BTreeMap<String, ProjectName>,
}
#[derive(Serialize, Deserialize)]
struct ProjectName {
    id: String,
    label: String,
    services: Vec<ServiceName>,
    /// Hour bucket, ms since the epoch. `0` is a registry written before stamps.
    #[serde(default)]
    last_seen_ms: i64,
}
#[derive(Serialize, Deserialize)]
struct ServiceName {
    id: String,
    fingerprint: String,
    label: String,
    /// Hour bucket, ms since the epoch. `0` is a registry written before stamps.
    #[serde(default)]
    last_seen_ms: i64,
}
const MAX_SERVICES_PER_PROJECT: usize = 32;
const MAX_SERVICES: usize = 2048;
/// Seen-at is stored as the start of the current hour, so a service that stays
/// up rewrites the registry at most hourly and a restart still finds it inside
/// the retention window.
const SEEN_BUCKET_MS: i64 = 60 * 60 * 1000;
const SERVICE_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;
static LABEL_TICK: AtomicU32 = AtomicU32::new(0);

pub fn slug(value: &str) -> String {
    let mut result = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            result.push(character.to_ascii_lowercase());
        } else if !result.ends_with('-') && !result.is_empty() {
            result.push('-');
        }
        if result.len() >= 48 {
            break;
        }
    }
    let result = result.trim_matches('-');
    if result.is_empty() {
        "project".into()
    } else {
        result.into()
    }
}

pub fn valid_hostname(host: &str) -> bool {
    let labels: Vec<_> = host.split('.').collect();
    labels.len() == 3
        && labels[2] == "localhost"
        && labels[..2].iter().all(|s| {
            !s.is_empty()
                && s.len() <= 63
                && !s.starts_with('-')
                && !s.ends_with('-')
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

fn allocate(base: &str, used: &HashSet<String>) -> String {
    let base = &base[..base.len().min(58)];
    let base = base.trim_end_matches('-');
    if !used.contains(base) {
        return base.to_owned();
    }
    for index in 2.. {
        let label = format!("{}-{index}", &base[..base.len().min(48)]);
        if !used.contains(&label) {
            return label;
        }
    }
    unreachable!()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn seen_bucket(now: i64) -> i64 {
    if now <= 0 {
        0
    } else {
        now - now.rem_euclid(SEEN_BUCKET_MS)
    }
}

/// `true` when the stored stamp moved into the current hour (so the file
/// should be rewritten). A stamp already in this hour, or newer, stays.
fn touch_seen(seen: &mut i64, now: i64) -> bool {
    let bucket = seen_bucket(now);
    if bucket == 0 || *seen >= bucket {
        return false;
    }
    *seen = bucket;
    true
}

fn used_labels_full(names: &Names) -> HashSet<String> {
    names
        .projects
        .values()
        .flat_map(|project| {
            std::iter::once(project.label.clone())
                .chain(project.services.iter().map(|service| service.label.clone()))
        })
        .collect()
}

fn verify_used_labels(used: &mut HashSet<String>, names: &Names) {
    if !crate::perf_verify(&LABEL_TICK) {
        return;
    }
    let full = used_labels_full(names);
    if *used != full {
        tracing::error!("preview used_labels diverged from the registry; resetting");
        *used = full;
    }
}

/// `last_seen_ms == 0` was written before stamps existed. Those names stay
/// until a scan stamps the services that are actually running; dropping them
/// at load would mint new hostnames for live previews. Once anything has been
/// stamped, an unstamped entry is older than the retention window.
fn registry_has_stamps(names: &Names) -> bool {
    names.projects.values().any(|project| {
        project.last_seen_ms > 0
            || project
                .services
                .iter()
                .any(|service| service.last_seen_ms > 0)
    })
}

fn expired(last_seen_ms: i64, now: i64, unstamped_are_stale: bool) -> bool {
    if last_seen_ms <= 0 {
        return unstamped_are_stale;
    }
    now.saturating_sub(last_seen_ms) > SERVICE_TTL_MS
}

fn drop_oldest(services: &mut Vec<ServiceName>, cap: usize, routed: &HashSet<String>) -> bool {
    if services.len() <= cap {
        return false;
    }
    let mut ranked: Vec<usize> = (0..services.len())
        .filter(|index| !routed.contains(&services[*index].id))
        .collect();
    ranked.sort_by(|&left, &right| {
        services[left]
            .last_seen_ms
            .cmp(&services[right].last_seen_ms)
            .then_with(|| services[left].id.cmp(&services[right].id))
    });
    let remove: HashSet<usize> = ranked.into_iter().take(services.len() - cap).collect();
    if remove.is_empty() {
        return false;
    }
    let mut index = 0;
    let before = services.len();
    services.retain(|_| {
        let drop = remove.contains(&index);
        index += 1;
        !drop
    });
    services.len() != before
}

fn drop_over_total(names: &mut Names, routed: &HashSet<String>) -> bool {
    let total: usize = names
        .projects
        .values()
        .map(|project| project.services.len())
        .sum();
    if total <= MAX_SERVICES {
        return false;
    }
    let mut ranked = Vec::new();
    for (cwd, project) in &names.projects {
        for service in &project.services {
            if !routed.contains(&service.id) {
                ranked.push((service.last_seen_ms, cwd.clone(), service.id.clone()));
            }
        }
    }
    ranked.sort();
    let drop_n = (total - MAX_SERVICES).min(ranked.len());
    let mut drop: HashMap<String, HashSet<String>> = HashMap::new();
    for (_, cwd, id) in ranked.into_iter().take(drop_n) {
        drop.entry(cwd).or_default().insert(id);
    }
    let mut changed = false;
    for (cwd, project) in names.projects.iter_mut() {
        let Some(ids) = drop.get(cwd) else {
            continue;
        };
        let before = project.services.len();
        project
            .services
            .retain(|service| !ids.contains(&service.id));
        changed |= project.services.len() != before;
    }
    changed
}

fn drop_missing_empty_projects(names: &mut Names) -> bool {
    let before = names.projects.len();
    names
        .projects
        .retain(|cwd, project| !project.services.is_empty() || Path::new(cwd).exists());
    names.projects.len() != before
}

/// Drops stale names. Routed service ids are kept even when they are the
/// oldest entries: their hostnames are already in use.
fn prune_names(names: &mut Names, routed: &HashSet<String>, now: i64) -> bool {
    let unstamped_are_stale = registry_has_stamps(names);
    let mut changed = false;
    for project in names.projects.values_mut() {
        let before = project.services.len();
        project.services.retain(|service| {
            routed.contains(&service.id) || !expired(service.last_seen_ms, now, unstamped_are_stale)
        });
        changed |= project.services.len() != before;
    }
    changed |= drop_missing_empty_projects(names);
    // An all-zero registry has not been scanned yet. Capping it would discard
    // an arbitrary subset and rename whichever of those services is still live.
    if unstamped_are_stale {
        for project in names.projects.values_mut() {
            changed |= drop_oldest(&mut project.services, MAX_SERVICES_PER_PROJECT, routed);
        }
        changed |= drop_over_total(names, routed);
        changed |= drop_missing_empty_projects(names);
    }
    changed
}

fn write_names(file: &Path, names: &Names) -> anyhow::Result<()> {
    let parent = file
        .parent()
        .context("preview registry has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    let temporary = file.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec(names)?)?;
    std::fs::rename(temporary, file)?;
    Ok(())
}

impl Catalog {
    pub fn open(
        file: impl Into<PathBuf>,
        device_id: String,
        device_name: String,
    ) -> anyhow::Result<Self> {
        let file = file.into();
        let mut names = match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice::<Names>(&bytes)
                .context("invalid preview identity registry")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Names::default(),
            Err(e) => return Err(e.into()),
        };
        if names.device_label.is_empty() {
            names.device_label = slug(&device_name);
        }
        let now = now_ms();
        let mut used_labels = used_labels_full(&names);
        if prune_names(&mut names, &HashSet::new(), now) {
            used_labels = used_labels_full(&names);
            write_names(&file, &names)?;
        }
        verify_used_labels(&mut used_labels, &names);
        let (changes, _) = watch::channel(PreviewSnapshot::default());
        Ok(Self(Arc::new(Inner {
            file,
            device_id,
            device_name,
            state: Mutex::new(State {
                names,
                routes: HashMap::new(),
                remote: HashMap::new(),
                proxy_port: PREVIEW_PROXY_PORT,
                error: None,
                used_labels,
            }),
            changes,
        })))
    }
    pub fn device_id(&self) -> &str {
        &self.0.device_id
    }
    pub fn subscribe(&self) -> watch::Receiver<PreviewSnapshot> {
        self.0.changes.subscribe()
    }
    pub(crate) fn watched(&self) -> bool {
        self.0.changes.receiver_count() > 0
    }
    pub fn snapshot(&self) -> PreviewSnapshot {
        self.0.changes.borrow().clone()
    }
    pub fn local_services(&self) -> Vec<PreviewService> {
        {
            let mut services: Vec<_> = self
                .0
                .state
                .lock()
                .unwrap()
                .routes
                .values()
                .map(|r| r.service.clone())
                .collect();
            services.sort_by(|a, b| a.id.cmp(&b.id));
            services
        }
    }
    pub fn local_route(&self, id: &str) -> Option<LocalRoute> {
        self.0.state.lock().unwrap().routes.get(id).cloned()
    }
    pub fn by_hostname(&self, hostname: &str) -> Option<PreviewService> {
        // Ambiguous remote aliases never pick an arbitrary machine. Local
        // aliases retain their established meaning when a peer has the same name.
        let state = self.0.state.lock().unwrap();
        if let Some(route) = state
            .routes
            .values()
            .find(|r| r.service.hostname == hostname)
        {
            return Some(route.service.clone());
        }
        let mut matches = state
            .remote
            .values()
            .flatten()
            .filter(|s| s.hostname == hostname);
        let result = matches.next()?.clone();
        matches.next().is_none().then_some(result)
    }
    pub fn set_proxy_status(&self, port: u16, error: Option<String>) {
        let mut state = self.0.state.lock().unwrap();
        state.proxy_port = port;
        state.error = error;
        self.publish(&state);
    }
    pub fn set_remote(
        &self,
        device: &str,
        mut services: Vec<PreviewService>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            device != self.device_id(),
            "cannot replace the local device catalog"
        );
        anyhow::ensure!(
            services.len() <= 256,
            "too many advertised preview services"
        );
        let mut ids = HashSet::new();
        anyhow::ensure!(
            services.iter().all(|s| s.device_id == device
                && valid_hostname(&s.hostname)
                && s.id.len() <= 128
                && !s.id.is_empty()
                && ids.insert(s.id.clone())
                && s.project_cwd.len() <= 4096
                && s.name.len() <= 128
                && s.chat_id.as_ref().is_none_or(|chat| chat.len() <= 128)),
            "invalid peer service advertisement"
        );
        let mut state = self.0.state.lock().unwrap();
        if let Some(first) = services.first() {
            if !state.names.peer_labels.contains_key(device) {
                let base = first.hostname.split('.').next().unwrap();
                let used = state
                    .names
                    .peer_labels
                    .values()
                    .cloned()
                    .chain(std::iter::once(state.names.device_label.clone()))
                    .collect();
                let label = allocate(base, &used);
                state.names.peer_labels.insert(device.to_owned(), label);
                self.persist(&state.names)?;
            }
            let label = &state.names.peer_labels[device];
            for service in &mut services {
                service.hostname =
                    format!("{label}.{}", service.hostname.split_once('.').unwrap().1);
            }
        }
        state.remote.insert(device.to_owned(), services);
        self.publish(&state);
        Ok(())
    }
    pub fn remove_remote(&self, device: &str) {
        let mut state = self.0.state.lock().unwrap();
        state.remote.remove(device);
        self.publish(&state);
    }
    pub fn clear_remote(&self) {
        let mut state = self.0.state.lock().unwrap();
        state.remote.clear();
        self.publish(&state);
    }
    fn persist(&self, names: &Names) -> anyhow::Result<()> {
        write_names(&self.0.file, names)
    }
    fn publish(&self, state: &State) {
        let mut services: Vec<_> = state
            .routes
            .values()
            .map(|r| r.service.clone())
            .chain(state.remote.values().flatten().cloned())
            .collect();
        services.sort_by(|a, b| {
            (
                &a.device_id,
                &a.project_cwd,
                !matches!(a.name.as_str(), "Vite" | "Next.js" | "Astro"),
                a.hostname.len(),
                &a.hostname,
            )
                .cmp(&(
                    &b.device_id,
                    &b.project_cwd,
                    !matches!(b.name.as_str(), "Vite" | "Next.js" | "Astro"),
                    b.hostname.len(),
                    &b.hostname,
                ))
        });
        let next = PreviewSnapshot {
            services,
            proxy_port: state.proxy_port,
            error: state.error.clone(),
            project_name: None,
            remote: false,
        };
        self.0.changes.send_if_modified(|previous| {
            if *previous == next {
                false
            } else {
                *previous = next;
                true
            }
        });
    }
    /// Input has already passed origin attribution and an HTTP handshake probe.
    pub fn replace_local(&self, mut observations: Vec<(PathBuf, Listener)>) -> anyhow::Result<()> {
        observations.sort_by_key(|(root, l)| {
            (
                root.clone(),
                std::cmp::Reverse(framework(&l.args).1),
                std::cmp::Reverse(l.zeron_owned),
                l.started_at,
                l.pid,
                l.address,
            )
        });
        let mut state = self.0.state.lock().unwrap();
        let now = now_ms();
        let mut next = HashMap::new();
        let mut seen_listeners = HashSet::new();
        let mut changed_names = false;
        for (root, listener) in observations.into_iter().take(256) {
            if !seen_listeners.insert((root.clone(), listener.address.port())) {
                continue;
            }
            let cwd = root.to_string_lossy().into_owned();
            if !state.names.projects.contains_key(&cwd) {
                let label = allocate(
                    &slug(
                        root.file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("project"),
                    ),
                    &state.used_labels,
                );
                state.used_labels.insert(label.clone());
                state.names.projects.insert(
                    cwd.clone(),
                    ProjectName {
                        id: uuid::Uuid::new_v4().to_string(),
                        label,
                        services: Vec::new(),
                        last_seen_ms: 0,
                    },
                );
                changed_names = true;
            }
            let fingerprint = format!(
                "{:x}",
                Sha256::digest(format!(
                    "{}\0{}",
                    listener.cwd.display(),
                    command_identity(
                        &listener
                            .args
                            .iter()
                            .filter(|arg| arg.parse::<u16>().ok() != Some(listener.address.port()))
                            .cloned()
                            .collect::<Vec<_>>()
                    )
                ))
            );
            let previous_id = state
                .routes
                .values()
                .find(|r| {
                    r.listener.pid == listener.pid
                        && r.listener.started_at == listener.started_at
                        && r.listener.address.port() == listener.address.port()
                })
                .map(|r| r.service.id.clone());
            let device_label = state.names.device_label.clone();
            let existing = {
                let project = state.names.projects.get(&cwd).unwrap();
                project
                    .services
                    .iter()
                    .find(|service| {
                        service.fingerprint == fingerprint
                            && previous_id.as_ref() == Some(&service.id)
                            && !next.contains_key(&service.id)
                    })
                    .or_else(|| {
                        project.services.iter().find(|service| {
                            service.fingerprint == fingerprint && !next.contains_key(&service.id)
                        })
                    })
                    .map(|service| service.id.clone())
            };
            let (framework_name, _) = framework(&listener.args);
            let role = service_role(&listener, framework_name);
            let id = if let Some(id) = existing {
                id
            } else {
                let (empty, base) = {
                    let project = state.names.projects.get(&cwd).unwrap();
                    if project.services.is_empty() {
                        (true, project.label.clone())
                    } else {
                        (false, format!("{}-{}", project.label, slug(&role)))
                    }
                };
                let label = if empty {
                    base
                } else {
                    allocate(&base, &state.used_labels)
                };
                state.used_labels.insert(label.clone());
                let id = uuid::Uuid::new_v4().to_string();
                state
                    .names
                    .projects
                    .get_mut(&cwd)
                    .unwrap()
                    .services
                    .push(ServiceName {
                        id: id.clone(),
                        fingerprint,
                        label,
                        last_seen_ms: 0,
                    });
                changed_names = true;
                id
            };
            let project = state.names.projects.get_mut(&cwd).unwrap();
            changed_names |= touch_seen(&mut project.last_seen_ms, now);
            let project_id = project.id.clone();
            let service_name = project
                .services
                .iter_mut()
                .find(|service| service.id == id)
                .unwrap();
            changed_names |= touch_seen(&mut service_name.last_seen_ms, now);
            let label = service_name.label.clone();
            let service = PreviewService {
                id: id.clone(),
                project_id,
                project_name: root
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                project_cwd: cwd,
                device_id: self.0.device_id.clone(),
                device_name: self.0.device_name.clone(),
                hostname: format!("{device_label}.{label}.localhost"),
                name: role,
                port: listener.address.port(),
                pid: listener.pid,
                cwd: listener.cwd.to_string_lossy().into_owned(),
                started_at: listener.started_at,
                zeron_owned: listener.zeron_owned,
                chat_id: listener.chat_id.clone(),
            };
            next.insert(id, LocalRoute { service, listener });
        }
        let routed: HashSet<String> = next.keys().cloned().collect();
        if prune_names(&mut state.names, &routed, now) {
            state.used_labels = used_labels_full(&state.names);
            changed_names = true;
        }
        if changed_names {
            self.persist(&state.names)?;
        }
        state.routes = next;
        {
            let State {
                used_labels, names, ..
            } = &mut *state;
            verify_used_labels(used_labels, names);
        }
        self.publish(&state);
        Ok(())
    }
}

fn service_role(listener: &Listener, framework_name: &str) -> String {
    if !matches!(framework_name, "HTTP server" | "Node HTTP server") {
        return framework_name.into();
    }
    for value in listener.args.iter().chain(std::iter::once(
        &listener.cwd.to_string_lossy().into_owned(),
    )) {
        let stem = Path::new(value)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(stem.as_str(), "api" | "api-server") {
            return "API".into();
        }
        if matches!(stem.as_str(), "docs" | "documentation") {
            return "Docs".into();
        }
    }
    framework_name.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn server(port: u16, pid: u32, command: &str) -> Listener {
        Listener {
            pid,
            parent: 1,
            cwd: "/work/my-app".into(),
            args: vec![
                "node".into(),
                command.into(),
                "--port".into(),
                port.to_string(),
            ],
            started_at: pid as u64,
            address: ([127, 0, 0, 1], port).into(),
            zeron_owned: true,
            origin: Some("chat:chat".into()),
            chat_id: Some("chat".into()),
        }
    }
    #[test]
    fn urls_survive_port_changes_daemon_restarts_and_multiple_services() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("previews.json");
        let catalog = Catalog::open(&file, "device-id".into(), "MacBook".into()).unwrap();
        let root = PathBuf::from("/work/my-app");
        catalog
            .replace_local(vec![
                (root.clone(), server(5173, 1, "vite")),
                (root.clone(), server(3000, 2, "api.js")),
            ])
            .unwrap();
        let before = catalog.snapshot();
        assert!(
            before
                .services
                .iter()
                .any(|s| s.hostname == "macbook.my-app.localhost" && s.port == 5173)
        );
        assert!(
            before
                .services
                .iter()
                .any(|s| s.hostname == "macbook.my-app-api.localhost" && s.port == 3000)
        );
        catalog.replace_local(Vec::new()).unwrap();
        assert!(catalog.snapshot().services.is_empty());
        drop(catalog);
        let catalog = Catalog::open(&file, "device-id".into(), "Renamed computer".into()).unwrap();
        catalog
            .replace_local(vec![
                (root.clone(), server(5174, 3, "vite")),
                (root, server(3001, 4, "api.js")),
            ])
            .unwrap();
        let after = catalog.snapshot();
        for old in before.services {
            let new = after.services.iter().find(|s| s.id == old.id).unwrap();
            assert_eq!(old.hostname, new.hostname);
            assert_ne!(old.port, new.port);
        }
    }
    #[test]
    fn duplicate_device_names_get_persistent_distinct_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("names.json");
        let catalog = Catalog::open(&file, "local".into(), "MacBook".into()).unwrap();
        catalog
            .replace_local(vec![("/work/my-app".into(), server(5173, 1, "vite"))])
            .unwrap();
        let mut remote = catalog.local_services()[0].clone();
        remote.device_id = "peer".into();
        catalog.set_remote("peer", vec![remote.clone()]).unwrap();
        assert_eq!(
            catalog
                .by_hostname("macbook.my-app.localhost")
                .unwrap()
                .device_id,
            "local"
        );
        assert_eq!(
            catalog
                .by_hostname("macbook-2.my-app.localhost")
                .unwrap()
                .device_id,
            "peer"
        );
        drop(catalog);
        let catalog = Catalog::open(&file, "local".into(), "MacBook".into()).unwrap();
        catalog.set_remote("peer", vec![remote]).unwrap();
        assert_eq!(
            catalog.snapshot().services[0].hostname,
            "macbook-2.my-app.localhost"
        );
    }
    #[test]
    fn hostnames_cannot_address_arbitrary_hosts() {
        for host in [
            "localhost",
            "foo.localhost.evil.test",
            "a..localhost",
            "a.b.localhost:80",
            "a.b.localhost/path",
            "a.b.c.localhost",
        ] {
            assert!(!valid_hostname(host));
        }
        assert!(valid_hostname("macbook.my-app-api.localhost"));
    }
    fn service_rows(catalog: &Catalog) -> Vec<(String, String)> {
        let state = catalog.0.state.lock().unwrap();
        let mut rows: Vec<_> = state
            .names
            .projects
            .values()
            .flat_map(|project| {
                project
                    .services
                    .iter()
                    .map(|service| (service.id.clone(), service.label.clone()))
            })
            .collect();
        rows.sort();
        rows
    }
    fn strip_seen(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Array(items) => {
                for item in items {
                    strip_seen(item);
                }
            }
            serde_json::Value::Object(map) => {
                map.remove("last_seen_ms");
                for item in map.values_mut() {
                    strip_seen(item);
                }
            }
            _ => {}
        }
    }
    #[test]
    fn subscription_marks_the_catalog_watched() {
        let temp = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(
            temp.path().join("previews.json"),
            "device".into(),
            "Mac".into(),
        )
        .unwrap();
        assert!(!catalog.watched());
        let receiver = catalog.subscribe();
        assert!(catalog.watched());
        drop(receiver);
        assert!(!catalog.watched());
    }
    #[test]
    fn prune_keeps_recent_and_routed_entries_and_drops_stale_ones() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("my-app");
        std::fs::create_dir(&root).unwrap();
        let catalog = Catalog::open(
            temp.path().join("previews.json"),
            "device".into(),
            "MacBook".into(),
        )
        .unwrap();
        let vite = server(5173, 1, "vite");
        let api = server(3000, 2, "api.js");
        catalog
            .replace_local(vec![
                (root.clone(), vite.clone()),
                (root.clone(), api.clone()),
            ])
            .unwrap();
        let before = catalog.snapshot();
        let vite_host = before
            .services
            .iter()
            .find(|s| s.port == 5173)
            .unwrap()
            .hostname
            .clone();
        let api_host = before
            .services
            .iter()
            .find(|s| s.port == 3000)
            .unwrap()
            .hostname
            .clone();
        {
            let mut state = catalog.0.state.lock().unwrap();
            let project = state
                .names
                .projects
                .get_mut(&*root.to_string_lossy())
                .unwrap();
            for service in &mut project.services {
                service.last_seen_ms = now_ms() - SERVICE_TTL_MS - 60_000;
            }
        }
        // The vite process is still routed, so its name survives the stale stamp.
        // The api process is not in this scan and is past the window.
        catalog
            .replace_local(vec![(root.clone(), vite.clone())])
            .unwrap();
        let left = catalog.snapshot();
        assert_eq!(left.services.len(), 1);
        assert_eq!(left.services[0].hostname, vite_host);
        assert_eq!(
            left.services[0].id,
            before.services.iter().find(|s| s.port == 5173).unwrap().id
        );
        assert!(
            !service_rows(&catalog)
                .iter()
                .any(|(_, label)| label == api_host.split('.').nth(1).unwrap())
        );
        // A recent service that is not in this scan stays reserved.
        catalog
            .replace_local(vec![
                (root.clone(), vite.clone()),
                (root.clone(), api.clone()),
            ])
            .unwrap();
        assert!(
            catalog
                .snapshot()
                .services
                .iter()
                .any(|s| s.hostname == api_host)
        );
        {
            let mut state = catalog.0.state.lock().unwrap();
            let project = state
                .names
                .projects
                .get_mut(&*root.to_string_lossy())
                .unwrap();
            let api_name = project
                .services
                .iter_mut()
                .find(|s| s.label != vite_host.split('.').nth(1).unwrap())
                .unwrap();
            api_name.last_seen_ms = now_ms() - 24 * 60 * 60 * 1000;
        }
        catalog.replace_local(vec![(root.clone(), vite)]).unwrap();
        assert!(
            service_rows(&catalog)
                .iter()
                .any(|(_, label)| label.contains("api"))
        );
        assert_eq!(catalog.snapshot().services[0].hostname, vite_host);

        let mut names = Names {
            device_label: "box".into(),
            ..Names::default()
        };
        let mut services = Vec::new();
        for index in 0..34 {
            services.push(ServiceName {
                id: format!("s{index:02}"),
                fingerprint: format!("f{index}"),
                label: format!("lab{index:02}"),
                last_seen_ms: 1_000 + index as i64,
            });
        }
        names.projects.insert(
            "/work/my-app".into(),
            ProjectName {
                id: "p".into(),
                label: "proj".into(),
                services,
                last_seen_ms: 50_000,
            },
        );
        names.projects.insert(
            "/no/such/zeron-preview-empty".into(),
            ProjectName {
                id: "g".into(),
                label: "gone".into(),
                services: Vec::new(),
                last_seen_ms: 50_000,
            },
        );
        let kept_dir = temp.path().join("still-here");
        std::fs::create_dir(&kept_dir).unwrap();
        names.projects.insert(
            kept_dir.to_string_lossy().into_owned(),
            ProjectName {
                id: "k".into(),
                label: "kept-empty".into(),
                services: Vec::new(),
                last_seen_ms: 50_000,
            },
        );
        let now = SERVICE_TTL_MS + 100_000;
        for (index, service) in names
            .projects
            .get_mut("/work/my-app")
            .unwrap()
            .services
            .iter_mut()
            .enumerate()
        {
            service.last_seen_ms = now - 40_000 + index as i64;
        }
        names.projects.get_mut("/work/my-app").unwrap().last_seen_ms = now;
        let routed = HashSet::from(["s00".to_string()]);
        assert!(prune_names(&mut names, &routed, now));
        let left: Vec<_> = names.projects["/work/my-app"]
            .services
            .iter()
            .map(|s| s.id.as_str())
            .collect();
        assert_eq!(left.len(), 32);
        assert!(
            left.contains(&"s00"),
            "a routed service is kept even when it is the oldest"
        );
        assert!(!left.contains(&"s01"));
        assert!(!left.contains(&"s02"));
        assert!(left.contains(&"s03"));
        assert!(!names.projects.contains_key("/no/such/zeron-preview-empty"));
        assert!(names.projects.contains_key(&*kept_dir.to_string_lossy()));
        // The same oldest service is dropped once nothing routes it.
        let project = names.projects.get_mut("/work/my-app").unwrap();
        for service in &mut project.services {
            if service.id == "s00" {
                service.last_seen_ms = now - SERVICE_TTL_MS - 1;
            }
        }
        assert!(prune_names(&mut names, &HashSet::new(), now));
        let left: Vec<_> = names.projects["/work/my-app"]
            .services
            .iter()
            .map(|s| s.id.as_str())
            .collect();
        assert_eq!(left.len(), 31);
        assert!(!left.contains(&"s00"));
    }
    #[test]
    fn used_labels_are_updated_on_insert_and_prune_not_rebuilt_per_scan() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("my-app");
        std::fs::create_dir(&root).unwrap();
        let catalog = Catalog::open(
            temp.path().join("previews.json"),
            "device".into(),
            "MacBook".into(),
        )
        .unwrap();
        catalog
            .replace_local(vec![
                (root.clone(), server(5173, 1, "vite")),
                (root.clone(), server(3000, 2, "api.js")),
            ])
            .unwrap();
        {
            let mut state = catalog.0.state.lock().unwrap();
            assert_eq!(state.used_labels, used_labels_full(&state.names));
            // A scan used to rebuild this set from names. A label that is not a
            // service name survives only when that rebuild does not run.
            state.used_labels.insert("my-app-docs".into());
        }
        catalog
            .replace_local(vec![
                (root.clone(), server(5173, 1, "vite")),
                (root.clone(), server(3000, 2, "api.js")),
                (root.clone(), server(4000, 3, "docs")),
            ])
            .unwrap();
        assert!(
            catalog
                .snapshot()
                .services
                .iter()
                .any(|service| service.hostname == "macbook.my-app-docs-2.localhost"),
            "allocation must see labels carried from the previous scan"
        );
        {
            let state = catalog.0.state.lock().unwrap();
            assert_eq!(state.used_labels, used_labels_full(&state.names));
            assert!(!state.used_labels.contains("my-app-docs"));
        }
        {
            let mut state = catalog.0.state.lock().unwrap();
            let project = state
                .names
                .projects
                .get_mut(&*root.to_string_lossy())
                .unwrap();
            let api = project
                .services
                .iter_mut()
                .find(|service| service.label.contains("api"))
                .unwrap();
            api.last_seen_ms = now_ms() - SERVICE_TTL_MS - 60_000;
        }
        catalog
            .replace_local(vec![
                (root.clone(), server(5173, 1, "vite")),
                (root.clone(), server(4000, 3, "docs")),
            ])
            .unwrap();
        assert!(
            !service_rows(&catalog)
                .iter()
                .any(|(_, label)| label.contains("api"))
        );
        catalog
            .replace_local(vec![
                (root.clone(), server(5173, 1, "vite")),
                (root.clone(), server(3001, 4, "api.js")),
                (root.clone(), server(4000, 3, "docs")),
            ])
            .unwrap();
        assert!(
            catalog
                .snapshot()
                .services
                .iter()
                .any(|service| service.hostname == "macbook.my-app-api.localhost")
        );
    }
    #[test]
    fn pretty_printed_registry_round_trips_and_later_writes_are_compact() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("previews.json");
        let catalog = Catalog::open(&file, "device-id".into(), "MacBook".into()).unwrap();
        let root = PathBuf::from("/work/my-app");
        catalog
            .replace_local(vec![(root.clone(), server(5173, 1, "vite"))])
            .unwrap();
        let service = catalog.snapshot().services[0].clone();
        let raw = std::fs::read(&file).unwrap();
        assert!(!raw.contains(&b'\n'));
        let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        strip_seen(&mut value);
        std::fs::write(&file, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        drop(catalog);
        let catalog = Catalog::open(&file, "device-id".into(), "Renamed computer".into()).unwrap();
        let rows = service_rows(&catalog);
        assert_eq!(rows, vec![(service.id.clone(), "my-app".into())]);
        catalog
            .replace_local(vec![(root, server(5174, 9, "vite"))])
            .unwrap();
        let again = &catalog.snapshot().services[0];
        assert_eq!(again.id, service.id);
        assert_eq!(again.hostname, service.hostname);
        assert_ne!(again.port, service.port);
        assert!(!std::fs::read(&file).unwrap().contains(&b'\n'));
    }
    #[test]
    fn a_forty_one_thousand_entry_registry_prunes_to_the_cap_and_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("previews.json");
        let kept_dir = temp.path().join("kept-empty");
        std::fs::create_dir(&kept_dir).unwrap();
        let now = now_ms();
        let mut names = Names {
            device_label: "macbook".into(),
            ..Names::default()
        };
        for project in 0..100 {
            let mut services = Vec::with_capacity(411);
            for service in 0..410 {
                let id = format!("s{project:03}-{service:03}");
                services.push(ServiceName {
                    id: id.clone(),
                    fingerprint: format!("f{project}-{service}"),
                    label: format!("lab-{id}"),
                    last_seen_ms: now - service as i64 * SEEN_BUCKET_MS,
                });
            }
            services.push(ServiceName {
                id: format!("stale-{project:03}"),
                fingerprint: "stale".into(),
                label: format!("stale-{project:03}"),
                last_seen_ms: now - SERVICE_TTL_MS - SEEN_BUCKET_MS,
            });
            names.projects.insert(
                format!("/no/such/preview/p{project:03}"),
                ProjectName {
                    id: format!("p{project:03}"),
                    label: format!("p{project:03}"),
                    services,
                    last_seen_ms: now,
                },
            );
        }
        names.projects.insert(
            "/no/such/preview-ghost".into(),
            ProjectName {
                id: "ghost".into(),
                label: "ghost".into(),
                services: Vec::new(),
                last_seen_ms: now,
            },
        );
        names.projects.insert(
            kept_dir.to_string_lossy().into_owned(),
            ProjectName {
                id: "kept".into(),
                label: "kept-empty".into(),
                services: Vec::new(),
                last_seen_ms: now,
            },
        );
        std::fs::write(&file, serde_json::to_vec(&names).unwrap()).unwrap();
        let catalog = Catalog::open(&file, "device".into(), "MacBook".into()).unwrap();
        let rows = service_rows(&catalog);
        assert_eq!(rows.len(), MAX_SERVICES);
        let ids: std::collections::BTreeSet<_> = rows.iter().map(|(id, _)| id.clone()).collect();
        assert!(ids.contains("s000-000"));
        assert!(ids.contains("s099-019"));
        assert!(ids.contains("s052-020"));
        assert!(ids.contains("s099-020"));
        assert!(!ids.contains("s000-020"));
        assert!(!ids.contains("s051-020"));
        assert!(!ids.contains("s000-031"));
        assert!(!ids.contains("s000-032"));
        assert!(!ids.iter().any(|id| id.starts_with("stale-")));
        for (id, label) in &rows {
            assert_eq!(label, &format!("lab-{id}"));
        }
        let state = catalog.0.state.lock().unwrap();
        assert!(!state.names.projects.contains_key("/no/such/preview-ghost"));
        assert!(
            state
                .names
                .projects
                .contains_key(&*kept_dir.to_string_lossy())
        );
        assert!(
            state
                .names
                .projects
                .values()
                .all(|project| project.services.len() <= MAX_SERVICES_PER_PROJECT)
        );
        assert_eq!(state.used_labels, used_labels_full(&state.names));
        drop(state);
        let bytes = std::fs::read(&file).unwrap();
        assert!(!bytes.contains(&b'\n'));
        drop(catalog);
        let catalog = Catalog::open(&file, "device".into(), "MacBook".into()).unwrap();
        assert_eq!(service_rows(&catalog), rows);
    }
}
