//! Engine-owned discovery/proxy lifecycle, independent of headed UI lifetimes.
use crate::{
    catalog::Catalog,
    discovery,
    mux::{self, BoxIo, Connector},
    peer::Peers,
    proxy, signaling,
};
use futures::{StreamExt, stream};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

/// Discovery must not turn into HTTP traffic against the user's dev servers.
/// A listening socket is probed until it answers HTTP once; that verdict then
/// holds for the socket's lifetime, because the process enumeration each
/// cycle already proves the same process still owns the same port. Sockets
/// that did not answer HTTP (a server bound but not yet serving) are retried
/// with a capped exponential backoff instead of on every cycle.
#[derive(Default)]
pub(crate) struct ProbeMemory {
    verdicts: HashMap<(u32, u64, SocketAddr), Verdict>,
}
enum Verdict {
    Http,
    NotHttp {
        retry_at: Instant,
        backoff: Duration,
    },
}
const NOT_HTTP_INITIAL_BACKOFF: Duration = Duration::from_secs(4);
const NOT_HTTP_MAX_BACKOFF: Duration = Duration::from_secs(60);

/// (servers already confirmed HTTP, listeners to probe this cycle)
type Planned<T> = (Vec<(T, discovery::Listener)>, Vec<(T, discovery::Listener)>);

fn probe_key(listener: &discovery::Listener) -> (u32, u64, SocketAddr) {
    (listener.pid, listener.started_at, listener.address)
}

impl ProbeMemory {
    /// Splits this cycle's candidates into servers already known to speak
    /// HTTP and listeners that need a probe now. Verdicts for sockets that
    /// are no longer listed are forgotten so a reused port is probed afresh.
    pub(crate) fn plan<T>(
        &mut self,
        candidates: Vec<(T, discovery::Listener)>,
        now: Instant,
    ) -> Planned<T> {
        let present: std::collections::HashSet<_> =
            candidates.iter().map(|(_, l)| probe_key(l)).collect();
        self.verdicts.retain(|key, _| present.contains(key));
        let mut known = Vec::new();
        let mut probe = Vec::new();
        for candidate in candidates {
            match self.verdicts.get(&probe_key(&candidate.1)) {
                Some(Verdict::Http) => known.push(candidate),
                Some(Verdict::NotHttp { retry_at, .. }) if *retry_at > now => {}
                _ => probe.push(candidate),
            }
        }
        (known, probe)
    }
    pub(crate) fn record(&mut self, listener: &discovery::Listener, http: bool, now: Instant) {
        let key = probe_key(listener);
        let verdict = if http {
            Verdict::Http
        } else {
            let backoff = match self.verdicts.get(&key) {
                Some(Verdict::NotHttp { backoff, .. }) => (*backoff * 2).min(NOT_HTTP_MAX_BACKOFF),
                _ => NOT_HTTP_INITIAL_BACKOFF,
            };
            Verdict::NotHttp {
                retry_at: now + backoff,
                backoff,
            }
        };
        self.verdicts.insert(key, verdict);
    }
}
#[derive(Clone)]
pub struct PreviewService(Arc<Inner>);
struct Inner {
    catalog: Catalog,
    stop: CancellationToken,
    started: AtomicBool,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}
pub type Projects = Arc<dyn Fn() -> Vec<PathBuf> + Send + Sync>;
impl PreviewService {
    pub fn new(file: PathBuf, device_id: String, device_name: String) -> anyhow::Result<Self> {
        Ok(Self(Arc::new(Inner {
            catalog: Catalog::open(file, device_id, device_name)?,
            stop: CancellationToken::new(),
            started: AtomicBool::new(false),
            tasks: Mutex::new(Vec::new()),
        })))
    }
    pub fn catalog(&self) -> &Catalog {
        &self.0.catalog
    }
    pub async fn start(&self, projects: Projects, signaling: Option<signaling::Config>) {
        if self.0.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let connector = Arc::new(LocalConnector(self.0.catalog.clone()));
        let local = mux::local(connector.clone(), self.0.stop.child_token());
        let (peers, output) = Peers::new(
            self.0.catalog.device_id().into(),
            connector,
            self.0.stop.child_token(),
        );
        let router = proxy::Router {
            catalog: self.0.catalog.clone(),
            local,
            peers: peers.clone(),
        };
        let proxy_catalog = self.0.catalog.clone();
        let proxy_stop = self.0.stop.clone();
        let listener_task = tokio::spawn(async move {
            loop {
                let binding = tokio::select! {
                    _ = proxy_stop.cancelled() => break,
                    binding = proxy::serve(router.clone(), zeron_proto::PREVIEW_PROXY_PORT, proxy_stop.child_token()) => binding,
                };
                match binding {
                    Ok((port, tasks)) => {
                        proxy_catalog.set_proxy_status(port, None);
                        for task in tasks {
                            let _ = task.await;
                        }
                        break;
                    }
                    Err(error) => proxy_catalog.set_proxy_status(
                        zeron_proto::PREVIEW_PROXY_PORT,
                        Some(format!(
                            "Local previews could not listen on port 7331: {error}"
                        )),
                    ),
                }
                tokio::select! { _ = proxy_stop.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
            }
        });
        self.0.tasks.lock().unwrap().push(listener_task);
        let catalog = self.0.catalog.clone();
        let stop = self.0.stop.clone();
        let scanner = tokio::spawn(async move {
            let mut memory = ProbeMemory::default();
            loop {
                let memory = &mut memory;
                let scan = async {
                    let projects = projects.clone();
                    let candidates = tokio::task::spawn_blocking(move || {
                        let mut roots: Vec<_> = projects()
                            .into_iter()
                            .collect::<std::collections::BTreeSet<_>>()
                            .into_iter()
                            .filter_map(|p| p.canonicalize().ok())
                            .collect::<std::collections::BTreeSet<_>>()
                            .into_iter()
                            .collect();
                        if roots.is_empty() {
                            return Vec::new();
                        }
                        roots.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
                        discovery::listeners()
                            .into_iter()
                            .filter(|l| {
                                l.address.port() != zeron_proto::PREVIEW_PROXY_PORT
                                    && l.pid != std::process::id()
                            })
                            .filter_map(|listener| {
                                roots
                                    .iter()
                                    .find(|root| listener.belongs_to(root))
                                    .cloned()
                                    .map(|root| (root, listener))
                            })
                            .collect::<Vec<_>>()
                    })
                    .await
                    .unwrap_or_default();
                    let (mut servers, probe) = memory.plan(candidates, Instant::now());
                    let probed: Vec<_> = stream::iter(probe)
                        .map(|(root, listener)| async move {
                            let http = discovery::is_http(listener.address).await;
                            (root, listener, http)
                        })
                        .buffer_unordered(12)
                        .collect()
                        .await;
                    let now = Instant::now();
                    for (root, listener, http) in probed {
                        memory.record(&listener, http, now);
                        if http {
                            servers.push((root, listener));
                        }
                    }
                    if let Err(error) = catalog.replace_local(servers) {
                        tracing::warn!(%error, "could not update preview services");
                    }
                };
                tokio::select! { _ = stop.cancelled() => break, _ = scan => {} }
                tokio::select! { _ = stop.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
            }
        });
        let mut tasks = self.0.tasks.lock().unwrap();
        tasks.push(scanner);
        if let Some(config) = signaling {
            tasks.push(tokio::spawn(signaling::run(
                config,
                self.0.catalog.clone(),
                peers,
                output,
                self.0.stop.child_token(),
            )));
        }
    }
    pub fn stop(&self) {
        self.0.stop.cancel();
        self.0.catalog.clear_remote();
    }
    pub async fn shutdown(&self) {
        self.stop();
        let tasks = std::mem::take(&mut *self.0.tasks.lock().unwrap());
        for task in tasks {
            let _ = task.await;
        }
    }
}
struct LocalConnector(Catalog);
#[async_trait::async_trait]
impl Connector for LocalConnector {
    async fn connect(&self, id: &str) -> anyhow::Result<BoxIo> {
        let route = self
            .0
            .local_route(id)
            .ok_or_else(|| anyhow::anyhow!("preview service stopped"))?;
        // Recheck process identity before dialing: a stopped dev server's port
        // may have been reused since the last scan. Checking the one pid is
        // cheap; enumerating every listener here spawned lsof and ps for each
        // asset a remote page requested, which showed up as memory and CPU
        // churn on the hosting Mac.
        let (pid, started_at) = (route.listener.pid, route.listener.started_at);
        let valid =
            tokio::task::spawn_blocking(move || discovery::same_process(pid, started_at)).await?;
        anyhow::ensure!(valid, "preview process changed; waiting for rediscovery");
        Ok(Box::new(
            tokio::net::TcpStream::connect(route.listener.address).await?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn listener(pid: u32, port: u16) -> discovery::Listener {
        discovery::Listener {
            pid,
            parent: 1,
            cwd: "/work/app".into(),
            args: vec!["node".into()],
            started_at: 1_000,
            address: ([127, 0, 0, 1], port).into(),
            zeron_owned: false,
        }
    }
    #[test]
    fn a_confirmed_http_server_is_never_probed_again() {
        let mut memory = ProbeMemory::default();
        let t0 = Instant::now();
        let (known, probe) = memory.plan(vec![((), listener(7, 8081))], t0);
        assert!(known.is_empty());
        assert_eq!(probe.len(), 1);
        memory.record(&probe[0].1, true, t0);
        for cycle in 1..1_000u64 {
            let now = t0 + Duration::from_secs(2 * cycle);
            let (known, probe) = memory.plan(vec![((), listener(7, 8081))], now);
            assert_eq!(known.len(), 1, "cycle {cycle} must reuse the verdict");
            assert!(probe.is_empty(), "cycle {cycle} must not send a request");
        }
    }
    #[test]
    fn non_http_listeners_back_off_and_reused_ports_are_probed_afresh() {
        let mut memory = ProbeMemory::default();
        let t0 = Instant::now();
        let (_, probe) = memory.plan(vec![((), listener(7, 8081))], t0);
        memory.record(&probe[0].1, false, t0);
        let (_, probe) = memory.plan(vec![((), listener(7, 8081))], t0 + Duration::from_secs(2));
        assert!(probe.is_empty(), "the very next cycle is skipped");
        let (_, probe) = memory.plan(vec![((), listener(7, 8081))], t0 + Duration::from_secs(5));
        assert_eq!(probe.len(), 1, "retried after the initial backoff");
        memory.record(&probe[0].1, false, t0 + Duration::from_secs(5));
        let (_, probe) = memory.plan(vec![((), listener(7, 8081))], t0 + Duration::from_secs(10));
        assert!(probe.is_empty(), "backoff doubled");
        // A different process on the same port carries no verdict over.
        let (_, probe) = memory.plan(vec![((), listener(8, 8081))], t0 + Duration::from_secs(10));
        assert_eq!(probe.len(), 1);
        // The original socket, once absent, is forgotten and re-probed on return.
        memory.record(&probe[0].1, true, t0);
        let (known, probe) =
            memory.plan(vec![((), listener(7, 8081))], t0 + Duration::from_secs(11));
        assert!(known.is_empty());
        assert_eq!(probe.len(), 1);
    }
    #[test]
    fn backoff_is_capped() {
        let mut memory = ProbeMemory::default();
        let mut now = Instant::now();
        for _ in 0..20 {
            memory.record(&listener(7, 8081), false, now);
            now += NOT_HTTP_MAX_BACKOFF;
        }
        let (_, probe) = memory.plan(vec![((), listener(7, 8081))], now);
        assert_eq!(probe.len(), 1, "still retried once per max backoff");
    }
}
