//! Persisted names are independent of ephemeral process/listener observations.
use crate::discovery::{Listener, command_identity, framework};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicU32},
    time::{Instant, SystemTime},
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
    internal_changes: watch::Sender<PreviewSnapshot>,
    scan_wake: tokio::sync::Notify,
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
    last_complete_scan: Option<Instant>,
    last_prune_bucket: Option<i64>,
    socket_index: SocketIndex,
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
    #[serde(default)]
    scan_clock_ms: i64,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SocketIdentity {
    inode: String,
    address: SocketAddr,
}

#[derive(Serialize, Deserialize)]
struct ServiceName {
    id: String,
    fingerprint: String,
    label: String,
    /// Hour bucket, ms since the epoch. `0` is a registry written before stamps.
    #[serde(default)]
    last_seen_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_socket: Option<SocketIdentity>,
}
/// Seen-at is stored as the start of the current hour, so a service that stays
/// up rewrites the registry at most hourly, including across restarts.
const SEEN_BUCKET_MS: i64 = 60 * 60 * 1000;
const SERVICE_TTL_MS: i64 = 30 * 24 * SEEN_BUCKET_MS;
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

#[derive(Default)]
struct SocketIndex {
    inodes: HashMap<String, Vec<(String, String)>>,
    addresses: HashMap<SocketAddr, Vec<(String, String)>>,
}
impl SocketIndex {
    fn build(names: &Names) -> Self {
        let mut index = Self::default();
        for (cwd, project) in &names.projects {
            for service in &project.services {
                if let Some(socket) = &service.last_socket {
                    let key = (cwd.clone(), service.id.clone());
                    index
                        .inodes
                        .entry(socket.inode.clone())
                        .or_default()
                        .push(key.clone());
                    index.addresses.entry(socket.address).or_default().push(key);
                }
            }
        }
        index
    }
    fn matches(&self, evidence: &crate::discovery::SocketEvidence) -> HashSet<(String, String)> {
        let mut result = HashSet::new();
        for (inode, address) in &evidence.sockets {
            if let Some(keys) = self.inodes.get(inode) {
                result.extend(keys.iter().cloned());
            }
            if let Some(keys) = self.addresses.get(address) {
                result.extend(keys.iter().cloned());
            }
        }
        result
    }
}

// A complete scan clock excludes downtime, failed scans, and partial scans.
// The extra hour compensates for rounding last-seen stamps down to an hour.
fn prune_names(
    names: &mut Names,
    routed: &HashSet<String>,
    now: i64,
    preserve_unknown: bool,
) -> bool {
    let mut changed = false;
    for project in names.projects.values_mut() {
        let before = project.services.len();
        project.services.retain(|service| {
            routed.contains(&service.id)
                || (preserve_unknown && service.last_socket.is_none())
                || now.saturating_sub(service.last_seen_ms) <= SERVICE_TTL_MS + SEEN_BUCKET_MS
        });
        changed |= before != project.services.len();
    }
    let before = names.projects.len();
    names
        .projects
        .retain(|cwd, project| !project.services.is_empty() || Path::new(cwd).exists());
    changed || before != names.projects.len()
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
        // Upgrade every legacy zero stamp independently. A registry written
        // before the scan clock existed has no evidence of completed scans,
        // so give all its entries a fresh retention window.
        let now = now_ms().max(1);
        let reset = names.scan_clock_ms <= 0;
        if reset {
            names.scan_clock_ms = now;
        }
        let mut migrated = reset;
        for project in names.projects.values_mut() {
            if reset || project.last_seen_ms <= 0 {
                project.last_seen_ms = names.scan_clock_ms.max(now);
                migrated = true;
            }
            for service in &mut project.services {
                if reset || service.last_seen_ms <= 0 {
                    service.last_seen_ms = names.scan_clock_ms.max(now);
                    migrated = true;
                }
            }
        }
        if migrated {
            write_names(&file, &names)?;
        }
        let socket_index = SocketIndex::build(&names);
        let mut used_labels = used_labels_full(&names);
        verify_used_labels(&mut used_labels, &names);
        let (changes, _) = watch::channel(PreviewSnapshot::default());
        let (internal_changes, _) = watch::channel(PreviewSnapshot::default());
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
                last_complete_scan: None,
                last_prune_bucket: None,
                socket_index,
            }),
            changes,
            internal_changes,
            scan_wake: tokio::sync::Notify::new(),
        })))
    }
    pub fn device_id(&self) -> &str {
        &self.0.device_id
    }
    pub fn subscribe(&self) -> watch::Receiver<PreviewSnapshot> {
        let receiver = self.0.changes.subscribe();
        self.0.scan_wake.notify_one();
        receiver
    }
    pub(crate) fn subscribe_internal(&self) -> watch::Receiver<PreviewSnapshot> {
        self.0.internal_changes.subscribe()
    }
    pub(crate) async fn scan_wake(&self) {
        self.0.scan_wake.notified().await;
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
        for changes in [&self.0.changes, &self.0.internal_changes] {
            changes.send_if_modified(|previous| {
                if *previous == next {
                    false
                } else {
                    *previous = next.clone();
                    true
                }
            });
        }
    }
    /// Input has already passed origin attribution and an HTTP handshake probe.
    pub fn replace_local(&self, observations: Vec<(PathBuf, Listener)>) -> anyhow::Result<()> {
        self.replace_local_scanned(observations, false)
    }
    pub(crate) fn scan_incomplete(&self) {
        self.0.state.lock().unwrap().last_complete_scan = None;
    }
    pub(crate) fn replace_local_scanned(
        &self,
        observations: Vec<(PathBuf, Listener)>,
        complete: bool,
    ) -> anyhow::Result<()> {
        self.replace_local_scanned_with_sockets(
            observations,
            complete,
            &crate::discovery::SocketEvidence::default(),
        )
    }
    pub(crate) fn replace_local_scanned_with_sockets(
        &self,
        mut observations: Vec<(PathBuf, Listener)>,
        complete: bool,
        evidence: &crate::discovery::SocketEvidence,
    ) -> anyhow::Result<()> {
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
        let complete = complete && observations.len() <= 256;
        let instant = Instant::now();
        let mut changed_names = false;
        let mut socket_index_dirty = false;
        if complete {
            if let Some(previous) = state.last_complete_scan {
                // Large scheduling gaps do not count as evidence of scans.
                let elapsed = instant
                    .saturating_duration_since(previous)
                    .min(std::time::Duration::from_secs(10));
                let before = seen_bucket(state.names.scan_clock_ms);
                state.names.scan_clock_ms = state
                    .names
                    .scan_clock_ms
                    .saturating_add(elapsed.as_millis() as i64);
                changed_names |= seen_bucket(state.names.scan_clock_ms) != before;
            }
            state.last_complete_scan = Some(instant);
        } else {
            state.last_complete_scan = None;
        }
        let now = state.names.scan_clock_ms;
        let protected_sockets = state.socket_index.matches(evidence);
        let mut next = HashMap::new();
        let mut seen_listeners = HashSet::new();
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
                let current_inode = evidence.owners.get(&(listener.pid, listener.address));
                let same_socket = |service: &ServiceName| {
                    current_inode.is_some()
                        && service.last_socket.as_ref().is_some_and(|socket| {
                            if evidence.sockets.contains_key(&socket.inode) {
                                current_inode == Some(&socket.inode)
                            } else {
                                socket.address == listener.address
                            }
                        })
                };
                let available = |service: &ServiceName| {
                    !next.contains_key(&service.id)
                        && (!protected_sockets.contains(&(cwd.clone(), service.id.clone()))
                            || same_socket(service))
                };
                project
                    .services
                    .iter()
                    .find(|service| available(service) && same_socket(service))
                    .or_else(|| {
                        project.services.iter().find(|service| {
                            available(service)
                                && service.fingerprint == fingerprint
                                && previous_id.as_ref() == Some(&service.id)
                        })
                    })
                    .or_else(|| {
                        project.services.iter().find(|service| {
                            available(service) && service.fingerprint == fingerprint
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
                        last_socket: None,
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
            if let Some(inode) = evidence.owners.get(&(listener.pid, listener.address)) {
                let socket = SocketIdentity {
                    inode: inode.clone(),
                    address: listener.address,
                };
                if service_name.last_socket.as_ref() != Some(&socket) {
                    service_name.last_socket = Some(socket);
                    changed_names = true;
                    socket_index_dirty = true;
                }
            }
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
        if socket_index_dirty {
            state.socket_index = SocketIndex::build(&state.names);
        }
        if complete {
            let maybe_live = state.socket_index.matches(evidence);
            for (cwd, id) in maybe_live {
                if let Some(project) = state.names.projects.get_mut(&cwd) {
                    if let Some(service) =
                        project.services.iter_mut().find(|service| service.id == id)
                    {
                        changed_names |= touch_seen(&mut service.last_seen_ms, now);
                    }
                }
            }
        }
        if complete && state.last_prune_bucket != Some(seen_bucket(now)) {
            // Old routes remain visible until persistence and publication succeed.
            let routed = next.keys().chain(state.routes.keys()).cloned().collect();
            // A legacy entry has no socket identity. An unreadable process
            // may still own any listening socket, so its name is ambiguous.
            let preserve_unknown = evidence.unreadable > 0 && !evidence.sockets.is_empty();
            if prune_names(&mut state.names, &routed, now, preserve_unknown) {
                state.used_labels = used_labels_full(&state.names);
                state.socket_index = SocketIndex::build(&state.names);
                changed_names = true;
            }
            state.last_prune_bucket = Some(seen_bucket(now));
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
        let internal = catalog.subscribe_internal();
        assert!(!catalog.watched());
        let receiver = catalog.subscribe();
        assert!(catalog.watched());
        drop(receiver);
        assert!(!catalog.watched());
        drop(internal);
    }
    #[test]
    fn old_names_survive_restart_partial_scans_and_caps() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("names.json");
        let root = temp.path().join("app");
        let catalog = Catalog::open(&file, "device".into(), "Mac".into()).unwrap();
        let live = server(5173, 1, "vite");
        catalog
            .replace_local(vec![(root.clone(), live.clone())])
            .unwrap();
        let original = catalog.snapshot().services[0].clone();
        {
            let mut state = catalog.0.state.lock().unwrap();
            let project = state
                .names
                .projects
                .get_mut(&*root.to_string_lossy())
                .unwrap();
            project.services[0].last_seen_ms = 1;
            for i in 0..2050 {
                project.services.push(ServiceName {
                    id: format!("other-{i}"),
                    fingerprint: format!("other-{i}"),
                    label: format!("other-{i}"),
                    last_seen_ms: 0,
                    last_socket: None,
                });
            }
            write_names(&file, &state.names).unwrap();
        }
        drop(catalog);
        let catalog = Catalog::open(&file, "device".into(), "Mac".into()).unwrap();
        catalog.replace_local(Vec::new()).unwrap();
        catalog.replace_local(vec![(root, live)]).unwrap();
        let restored = &catalog.snapshot().services[0];
        assert_eq!(restored.id, original.id);
        assert_eq!(restored.hostname, original.hostname);
        assert_eq!(service_rows(&catalog).len(), 2051);
    }
    #[test]
    fn completed_scan_expiry_keeps_routed_and_recent_names() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("names.json");
        let root = temp.path().join("app");
        let catalog = Catalog::open(&file, "device".into(), "Mac".into()).unwrap();
        let live = server(5173, 1, "vite");
        let old = server(3000, 2, "api.js");
        catalog
            .replace_local(vec![(root.clone(), live.clone()), (root.clone(), old)])
            .unwrap();
        let before = catalog.snapshot().services;
        let old_id = before.iter().find(|s| s.port == 3000).unwrap().id.clone();
        let live_entry = before.iter().find(|s| s.port == 5173).unwrap().clone();
        {
            let mut state = catalog.0.state.lock().unwrap();
            state.names.scan_clock_ms += SERVICE_TTL_MS + 2 * SEEN_BUCKET_MS;
        }
        // Partial scans cannot prune, even after sufficient completed-scan time.
        catalog
            .replace_local_scanned(vec![(root.clone(), live.clone())], false)
            .unwrap();
        assert!(service_rows(&catalog).iter().any(|(id, _)| *id == old_id));
        catalog
            .replace_local_scanned(vec![(root.clone(), live.clone())], true)
            .unwrap();
        assert!(!service_rows(&catalog).iter().any(|(id, _)| *id == old_id));
        let after = catalog.snapshot().services[0].clone();
        assert_eq!(
            (after.id, after.hostname),
            (live_entry.id, live_entry.hostname)
        );
        drop(catalog);
        let reopened = Catalog::open(&file, "device".into(), "Mac".into()).unwrap();
        reopened
            .replace_local_scanned(vec![(root, live)], true)
            .unwrap();
        assert_eq!(
            reopened.snapshot().services[0].id,
            before.iter().find(|s| s.port == 5173).unwrap().id
        );
    }
    #[test]
    fn pruning_protects_routed_names_even_if_their_stamps_are_old() {
        let mut names = Names::default();
        names.projects.insert(
            "/missing".into(),
            ProjectName {
                id: "project".into(),
                label: "app".into(),
                last_seen_ms: 1,
                services: vec![ServiceName {
                    id: "live".into(),
                    fingerprint: "f".into(),
                    label: "app".into(),
                    last_seen_ms: 1,
                    last_socket: None,
                }],
            },
        );
        assert!(!prune_names(
            &mut names,
            &HashSet::from(["live".into()]),
            SERVICE_TTL_MS + 2 * SEEN_BUCKET_MS,
            false
        ));
        assert_eq!(names.projects["/missing"].services[0].id, "live");
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn inaccessible_fake_proc_processes_are_counted_and_scan_clock_advances() {
        let temp = tempfile::tempdir().unwrap();
        let proc_root = temp.path().join("proc");
        for pid in [1001, 1002, 1003] {
            let path = proc_root.join(pid.to_string());
            std::fs::create_dir_all(path.join("fd")).unwrap();
            let fields = std::iter::once("S")
                .chain(std::iter::once("1"))
                .chain(std::iter::repeat_n("0", 17))
                .chain(std::iter::once("100"))
                .collect::<Vec<_>>()
                .join(" ");
            std::fs::write(path.join("stat"), format!("{pid} (test) {fields}\n")).unwrap();
        }
        let scan = |errno| {
            let mut origins = crate::discovery::Origins::default();
            crate::discovery::listeners_from(
                &mut origins,
                &HashMap::new(),
                &proc_root,
                &mut |path| match path
                    .parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                {
                    "1002" => Err(std::io::Error::from_raw_os_error(errno)),
                    "1003" => Err(std::io::Error::from_raw_os_error(libc::ENOENT)),
                    _ => std::fs::read_dir(path),
                },
            )
            .unwrap()
        };
        let (_, complete, unreadable, _) = scan(libc::EACCES);
        assert!(complete);
        assert_eq!(unreadable, 1);
        let (_, complete_ep, unreadable_ep, _) = scan(libc::EPERM);
        assert!(complete_ep);
        assert_eq!(unreadable_ep, 1);
        let (_, complete_io, _, _) = scan(libc::EIO);
        assert!(!complete_io);

        let catalog = Catalog::open(
            temp.path().join("names.json"),
            "device".into(),
            "Mac".into(),
        )
        .unwrap();
        catalog.replace_local_scanned(Vec::new(), complete).unwrap();
        {
            catalog.0.state.lock().unwrap().last_complete_scan =
                Some(Instant::now() - std::time::Duration::from_secs(5));
        }
        let before = catalog.0.state.lock().unwrap().names.scan_clock_ms;
        catalog.replace_local_scanned(Vec::new(), complete).unwrap();
        assert!(catalog.0.state.lock().unwrap().names.scan_clock_ms >= before + 5_000);
    }
    #[test]
    fn scan_clock_pauses_for_partial_scans_and_restarts() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("names.json");
        let catalog = Catalog::open(&file, "device".into(), "Mac".into()).unwrap();
        catalog.replace_local_scanned(Vec::new(), true).unwrap();
        {
            catalog.0.state.lock().unwrap().last_complete_scan =
                Some(Instant::now() - std::time::Duration::from_secs(5));
        }
        let before = catalog.0.state.lock().unwrap().names.scan_clock_ms;
        catalog.replace_local_scanned(Vec::new(), false).unwrap();
        catalog.replace_local_scanned(Vec::new(), true).unwrap();
        assert_eq!(catalog.0.state.lock().unwrap().names.scan_clock_ms, before);
        {
            catalog.0.state.lock().unwrap().last_complete_scan =
                Some(Instant::now() - std::time::Duration::from_secs(5));
        }
        catalog.replace_local_scanned(Vec::new(), true).unwrap();
        assert!(catalog.0.state.lock().unwrap().names.scan_clock_ms >= before + 5_000);
        drop(catalog);
        let catalog = Catalog::open(&file, "device".into(), "Mac".into()).unwrap();
        let before = catalog.0.state.lock().unwrap().names.scan_clock_ms;
        catalog.replace_local_scanned(Vec::new(), true).unwrap();
        assert_eq!(catalog.0.state.lock().unwrap().names.scan_clock_ms, before);
    }
    #[test]
    fn socket_identity_matches_inode_or_address_fallback() {
        let address = ([127, 0, 0, 1], 5173).into();
        let mut names = Names::default();
        names.projects.insert(
            "/app".into(),
            ProjectName {
                id: "p".into(),
                label: "app".into(),
                last_seen_ms: 1,
                services: vec![ServiceName {
                    id: "s".into(),
                    fingerprint: "f".into(),
                    label: "app".into(),
                    last_seen_ms: 1,
                    last_socket: Some(SocketIdentity {
                        inode: "123".into(),
                        address,
                    }),
                }],
            },
        );
        let index = SocketIndex::build(&names);
        let expected = HashSet::from([("/app".into(), "s".into())]);
        let fallback = crate::discovery::SocketEvidence {
            sockets: HashMap::from([("456".into(), address)]),
            ..Default::default()
        };
        assert_eq!(index.matches(&fallback), expected);
        let same_inode = crate::discovery::SocketEvidence {
            sockets: HashMap::from([("123".into(), ([127, 0, 0, 1], 9999).into())]),
            ..Default::default()
        };
        assert_eq!(index.matches(&same_inode), expected);
        assert!(
            index
                .matches(&crate::discovery::SocketEvidence::default())
                .is_empty()
        );
    }
    #[test]
    fn legacy_unknown_socket_names_are_kept_while_ownership_is_ambiguous() {
        let temp = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(
            temp.path().join("names.json"),
            "device".into(),
            "Mac".into(),
        )
        .unwrap();
        let root = temp.path().to_path_buf();
        catalog
            .replace_local(vec![(root, server(5173, 1, "vite"))])
            .unwrap();
        let id = catalog.snapshot().services[0].id.clone();
        {
            let mut state = catalog.0.state.lock().unwrap();
            state.names.scan_clock_ms += SERVICE_TTL_MS + 2 * SEEN_BUCKET_MS;
        }
        // The unknown legacy name could belong to an inaccessible listener.
        let ambiguous = crate::discovery::SocketEvidence {
            sockets: HashMap::from([("unknown".into(), ([127, 0, 0, 1], 5173).into())]),
            unreadable: 1,
            ..Default::default()
        };
        catalog
            .replace_local_scanned_with_sockets(Vec::new(), true, &ambiguous)
            .unwrap();
        catalog
            .replace_local_scanned_with_sockets(Vec::new(), true, &ambiguous)
            .unwrap();
        assert!(service_rows(&catalog).iter().any(|(name, _)| name == &id));
        // Once no inaccessible owner exists, a later complete scan may expire it.
        {
            let mut state = catalog.0.state.lock().unwrap();
            state.names.scan_clock_ms += SEEN_BUCKET_MS;
        }
        catalog
            .replace_local_scanned_with_sockets(
                Vec::new(),
                true,
                &crate::discovery::SocketEvidence::default(),
            )
            .unwrap();
        assert!(!service_rows(&catalog).iter().any(|(name, _)| name == &id));
    }
    #[test]
    fn a_live_unreadable_socket_cannot_be_reassigned_to_a_matching_new_server() {
        let temp = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(
            temp.path().join("names.json"),
            "device".into(),
            "Mac".into(),
        )
        .unwrap();
        let root = temp.path().to_path_buf();
        let first = server(5173, 1, "vite");
        let second = server(5174, 2, "vite");
        let initial = crate::discovery::SocketEvidence {
            sockets: HashMap::from([("first".into(), first.address)]),
            owners: HashMap::from([((first.pid, first.address), "first".into())]),
            unreadable: 0,
        };
        catalog
            .replace_local_scanned_with_sockets(vec![(root.clone(), first.clone())], true, &initial)
            .unwrap();
        let original = catalog.snapshot().services[0].clone();
        let both = crate::discovery::SocketEvidence {
            sockets: HashMap::from([
                ("first".into(), first.address),
                ("second".into(), second.address),
            ]),
            owners: HashMap::from([((second.pid, second.address), "second".into())]),
            unreadable: 1,
        };
        catalog
            .replace_local_scanned_with_sockets(vec![(root.clone(), second)], true, &both)
            .unwrap();
        assert_ne!(catalog.snapshot().services[0].id, original.id);
        assert!(
            service_rows(&catalog)
                .iter()
                .any(|(id, label)| id == &original.id && original.hostname.contains(label))
        );
        catalog
            .replace_local_scanned_with_sockets(vec![(root, first)], true, &initial)
            .unwrap();
        assert_eq!(catalog.snapshot().services[0].id, original.id);
        assert_eq!(catalog.snapshot().services[0].hostname, original.hostname);
    }
    #[cfg(target_os = "linux")]
    fn assert_unreadable_socket_keeps_name(transfer: bool) {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                // Also reap a forked socket holder if an assertion fails.
                if self.0.try_wait().ok().flatten().is_none() {
                    unsafe {
                        libc::kill(-(self.0.id() as i32), libc::SIGKILL);
                    }
                    let _ = self.0.wait();
                }
            }
        }
        let script = r#"import ctypes,os,socket,sys,time
s=socket.socket(); s.bind(('127.0.0.1',0)); s.listen()
print(s.getsockname()[1], flush=True)
sys.stdin.readline()
if sys.argv[1] == 'transfer':
    r,w=os.pipe(); pid=os.fork()
    if pid == 0:
        os.close(r); assert ctypes.CDLL(None).prctl(4,0,0,0,0)==0
        os.write(w,b'1'); os.close(w); time.sleep(120); os._exit(0)
    os.close(w); os.read(r,1); os.close(r); s.close()
else:
    assert ctypes.CDLL(None).prctl(4,0,0,0,0)==0
print('hidden', pid if sys.argv[1] == 'transfer' else os.getpid(), flush=True)
sys.stdin.readline()
if sys.argv[1] == 'transfer': os.kill(pid,9); os.waitpid(pid,0)
"#;
        let mut child = Child(
            Command::new("python3")
                .args([
                    "-u",
                    "-c",
                    script,
                    if transfer { "transfer" } else { "hide" },
                ])
                .process_group(0)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut output = BufReader::new(child.0.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let port: u16 = line.trim().parse().unwrap();
        let mut origins = crate::discovery::Origins::default();
        let (listeners, _, _, before) =
            crate::discovery::listeners_with_completeness(&mut origins).unwrap();
        let listener = listeners
            .into_iter()
            .find(|listener| listener.pid == child.0.id() && listener.address.port() == port)
            .expect("initial owner");
        let inode = before.owners[&(listener.pid, listener.address)].clone();
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("names.json");
        let root = temp.path().to_path_buf();
        let catalog = Catalog::open(&file, "device".into(), "Mac".into()).unwrap();
        catalog
            .replace_local_scanned_with_sockets(vec![(root.clone(), listener)], true, &before)
            .unwrap();
        let original = catalog.snapshot().services[0].clone();
        writeln!(child.0.stdin.as_mut().unwrap(), "hide").unwrap();
        line.clear();
        output.read_line(&mut line).unwrap();
        let mut hidden = line.split_whitespace();
        assert_eq!(hidden.next(), Some("hidden"));
        let hidden_pid: u32 = hidden.next().unwrap().parse().unwrap();
        let catalog = {
            drop(catalog);
            Catalog::open(&file, "device".into(), "Mac".into()).unwrap()
        };
        for _ in 0..5 {
            // The test user namespace may have ptrace capabilities over its
            // child. Inject the denied fd read while using the real socket table.
            let evidence = (0..20)
                .find_map(|_| {
                    let (_, stable, _, mut evidence) =
                        crate::discovery::listeners_with_completeness(&mut origins)?;
                    let (_, complete, skipped, owners) = crate::discovery::listeners_from(
                        &mut origins,
                        &evidence.sockets,
                        Path::new("/proc"),
                        &mut |path| {
                            if path
                                .parent()
                                .and_then(Path::file_name)
                                .and_then(|n| n.to_str())
                                .and_then(|s| s.parse::<u32>().ok())
                                == Some(hidden_pid)
                            {
                                Err(std::io::Error::from_raw_os_error(libc::EACCES))
                            } else {
                                std::fs::read_dir(path)
                            }
                        },
                    )?;
                    if !stable || !complete || skipped == 0 {
                        return None;
                    }
                    evidence.owners = owners;
                    Some(evidence)
                })
                .expect("complete socket and process scans");
            assert!(evidence.sockets.contains_key(&inode));
            assert!(
                !evidence
                    .owners
                    .keys()
                    .any(|(pid, address)| *pid == hidden_pid && address.port() == original.port)
            );
            {
                let mut state = catalog.0.state.lock().unwrap();
                state.names.scan_clock_ms += SERVICE_TTL_MS / 2;
            }
            catalog
                .replace_local_scanned_with_sockets(Vec::new(), true, &evidence)
                .unwrap();
            assert!(
                service_rows(&catalog)
                    .iter()
                    .any(|(id, _)| *id == original.id)
            );
        }
        writeln!(child.0.stdin.as_mut().unwrap(), "stop").unwrap();
        assert!(child.0.wait().unwrap().success());
        let (_, _, _, evidence) =
            crate::discovery::listeners_with_completeness(&mut origins).unwrap();
        assert!(!evidence.sockets.contains_key(&inode));
        {
            let mut state = catalog.0.state.lock().unwrap();
            state.names.scan_clock_ms += SERVICE_TTL_MS + 2 * SEEN_BUCKET_MS;
        }
        catalog
            .replace_local_scanned_with_sockets(Vec::new(), true, &evidence)
            .unwrap();
        assert!(
            !service_rows(&catalog)
                .iter()
                .any(|(id, _)| *id == original.id)
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn a_named_server_that_becomes_non_dumpable_keeps_its_name() {
        assert_unreadable_socket_keeps_name(false);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn a_socket_handed_to_an_unreadable_process_keeps_its_name() {
        assert_unreadable_socket_keeps_name(true);
    }
    #[test]
    fn seen_stamps_are_hourly_and_survive_restart() {
        let mut stamp = 0;
        let hour = 500 * SEEN_BUCKET_MS;
        assert!(touch_seen(&mut stamp, hour + 1));
        assert_eq!(stamp, hour);
        let serialized = serde_json::to_vec(&stamp).unwrap();
        let mut stamp: i64 = serde_json::from_slice(&serialized).unwrap();
        assert!(!touch_seen(&mut stamp, hour + SEEN_BUCKET_MS - 1));
        assert!(touch_seen(&mut stamp, hour + SEEN_BUCKET_MS));
        assert_eq!(stamp, hour + SEEN_BUCKET_MS);
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
    fn legacy_forty_one_thousand_entries_survive_partial_migration() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("previews.json");
        let root = temp.path().join("app");
        let mut names = Names {
            device_label: "mac".into(),
            ..Names::default()
        };
        let services = (0..41_000)
            .map(|i| ServiceName {
                id: format!("id-{i}"),
                fingerprint: format!("fp-{i}"),
                label: format!("label-{i}"),
                last_seen_ms: 0,
                last_socket: None,
            })
            .collect();
        names.projects.insert(
            root.to_string_lossy().into_owned(),
            ProjectName {
                id: "project".into(),
                label: "app".into(),
                services,
                last_seen_ms: 0,
            },
        );
        std::fs::write(&file, serde_json::to_vec_pretty(&names).unwrap()).unwrap();
        let catalog = Catalog::open(&file, "device".into(), "Mac".into()).unwrap();
        {
            let state = catalog.0.state.lock().unwrap();
            assert!(
                state
                    .names
                    .projects
                    .values()
                    .flat_map(|p| &p.services)
                    .all(|s| s.last_seen_ms > 0)
            );
        }
        catalog
            .replace_local(vec![(root, server(5173, 1, "vite"))])
            .unwrap();
        assert_eq!(service_rows(&catalog).len(), 41_001);
        drop(catalog);
        let reopened = Catalog::open(&file, "device".into(), "Mac".into()).unwrap();
        assert_eq!(service_rows(&reopened).len(), 41_001);
    }
}
