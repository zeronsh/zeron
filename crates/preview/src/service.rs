//! Engine-owned discovery/proxy lifecycle, independent of headed UI lifetimes.
use crate::{
    catalog::Catalog,
    discovery,
    login::{self, CallbackForwarder, CallbackRoutes, CallbackTunnels},
    mux::{self, BoxIo, Connector},
    peer::Peers,
    proxy, signaling,
};
use futures::{StreamExt, stream};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    future::Future,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// Rescan cadence while anything Zeron spawned is alive. A pass walks only
/// those processes through libproc (tens of syscalls, well under a
/// millisecond for a typical agent tree) and touches the chat registry and
/// the filesystem only when a listening socket turns up, so 2 s keeps a new
/// dev server appearing promptly for a negligible cost. With nothing alive
/// there is no pass and no timer at all.
const SCAN_INTERVAL: Duration = Duration::from_secs(2);
/// A chat folder that could not be resolved (deleted worktree, unmounted
/// volume) is retried this often rather than on every pass.
const RETRY_UNRESOLVED_ROOT: Duration = Duration::from_secs(30);

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
    /// Sign-in callbacks peers may reach on this device (see [`login`]).
    callback_routes: CallbackRoutes,
    /// Sign-in callbacks this device forwards to the device running them.
    callback_tunnels: CallbackTunnels,
    peers: std::sync::OnceLock<Peers>,
    /// Wakes a parked discovery loop (a run or terminal just started).
    discovery_wake: Arc<Notify>,
    busy: OnceLock<Busy>,
}
/// Folders of this device's live (non-archived) chats.
pub type Projects = Arc<dyn Fn() -> Vec<PathBuf> + Send + Sync>;
/// Whether the engine has work in flight whose processes may not exist yet
/// (a run being dispatched): discovery keeps polling across that gap.
pub type Busy = Arc<dyn Fn() -> bool + Send + Sync>;

/// Canonical project roots, resolved once per distinct folder instead of
/// canonicalizing every chat's folder on every pass.
#[derive(Default)]
pub(crate) struct ProjectRoots {
    resolved: HashMap<PathBuf, Result<PathBuf, Instant>>,
}
impl ProjectRoots {
    /// Deepest first, so a listener in a nested project matches that one.
    pub(crate) fn resolve(&mut self, folders: Vec<PathBuf>, now: Instant) -> Vec<PathBuf> {
        let folders: HashSet<_> = folders.into_iter().collect();
        self.resolved.retain(|folder, _| folders.contains(folder));
        for folder in folders {
            let stale = match self.resolved.get(&folder) {
                None => true,
                Some(Err(failed)) => now.duration_since(*failed) >= RETRY_UNRESOLVED_ROOT,
                Some(Ok(_)) => false,
            };
            if stale {
                let resolved = folder.canonicalize().map_err(|_| now);
                self.resolved.insert(folder, resolved);
            }
        }
        let mut roots: Vec<_> = self
            .resolved
            .values()
            .filter_map(|r| r.as_ref().ok().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        roots.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
        roots
    }
}

/// Runs passes while `pass` reports live processes (or the engine is busy)
/// and parks — no pass, no timer — until `wake` otherwise. The first pass
/// runs at once, so a service started under live children finds them.
pub(crate) async fn discovery_loop<P, F>(
    mut pass: P,
    busy: impl Fn() -> bool,
    wake: Arc<Notify>,
    stop: CancellationToken,
) where
    P: FnMut() -> F,
    F: Future<Output = bool>,
{
    loop {
        let live = tokio::select! { _ = stop.cancelled() => return, live = pass() => live };
        if live || busy() {
            tokio::select! { _ = stop.cancelled() => return, _ = tokio::time::sleep(SCAN_INTERVAL) => {} }
        } else {
            tokio::select! { _ = stop.cancelled() => return, _ = wake.notified() => {} }
        }
    }
}

/// One discovery pass's state, shared with the blocking pool.
#[derive(Clone)]
struct Scanner {
    catalog: Catalog,
    projects: Projects,
    root: u32,
    blocking: Arc<Mutex<(discovery::Descendants, ProjectRoots)>>,
    memory: Arc<Mutex<ProbeMemory>>,
}
impl Scanner {
    /// Publishes this pass's servers; returns whether anything below the
    /// root is still alive.
    async fn pass(self) -> bool {
        let this = self;
        let (projects, blocking, root) = (this.projects.clone(), this.blocking.clone(), this.root);
        let (live, candidates) = tokio::task::spawn_blocking(move || {
            let mut state = blocking.lock().unwrap_or_else(PoisonError::into_inner);
            let (descendants, roots) = &mut *state;
            let scan = descendants.scan(root);
            let own = std::process::id();
            let listeners: Vec<_> = scan
                .listeners
                .into_iter()
                .filter(|l| {
                    l.address.port() != zeron_proto::PREVIEW_PROXY_PORT
                        && l.pid != own
                        && !l.is_authentication_command()
                })
                .collect();
            if listeners.is_empty() {
                return (scan.live, Vec::new());
            }
            let roots = roots.resolve(projects(), Instant::now());
            let candidates = listeners
                .into_iter()
                .filter_map(|listener| {
                    roots
                        .iter()
                        .find(|root| listener.belongs_to(root))
                        .cloned()
                        .map(|root| (root, listener))
                })
                .collect::<Vec<_>>();
            (scan.live, candidates)
        })
        .await
        .unwrap_or_default();
        let (mut servers, probe) = this
            .memory
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .plan(candidates, Instant::now());
        let probed: Vec<_> = stream::iter(probe)
            .map(|(root, listener)| async move {
                let http = discovery::is_http(listener.address).await;
                (root, listener, http)
            })
            .buffer_unordered(12)
            .collect()
            .await;
        let now = Instant::now();
        let mut memory = this.memory.lock().unwrap_or_else(PoisonError::into_inner);
        for (root, listener, http) in probed {
            memory.record(&listener, http, now);
            if http {
                servers.push((root, listener));
            }
        }
        drop(memory);
        // An empty pass (the last child exited) withdraws its servers.
        if let Err(error) = this.catalog.replace_local(servers) {
            tracing::warn!(%error, "could not update preview services");
        }
        live
    }
}
impl PreviewService {
    pub fn new(file: PathBuf, device_id: String, device_name: String) -> anyhow::Result<Self> {
        let catalog = Catalog::open(file, device_id, device_name)?;
        Ok(Self(Arc::new(Inner {
            callback_routes: CallbackRoutes::default(),
            catalog,
            stop: CancellationToken::new(),
            started: AtomicBool::new(false),
            tasks: Mutex::new(Vec::new()),
            callback_tunnels: CallbackTunnels::default(),
            peers: std::sync::OnceLock::new(),
            discovery_wake: Arc::new(Notify::new()),
            busy: OnceLock::new(),
        })))
    }
    /// A run or terminal may have just started: resume discovery if it is
    /// parked. Cheap and safe to call often.
    pub fn wake_discovery(&self) {
        self.0.discovery_wake.notify_one();
    }
    /// Resume discovery whenever `changes` does (runs or terminals
    /// starting), until the service stops.
    pub fn wake_discovery_on<T: Send + Sync + 'static>(
        &self,
        mut changes: tokio::sync::watch::Receiver<T>,
    ) {
        let (wake, stop) = (self.0.discovery_wake.clone(), self.0.stop.clone());
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    changed = changes.changed() => match changed {
                        Ok(()) => wake.notify_one(),
                        Err(_) => break,
                    },
                }
            }
        });
        self.0.tasks.lock().unwrap().push(task);
    }
    /// See [`Busy`]. Set before [`Self::start`]; first wins.
    pub fn set_busy(&self, busy: Busy) {
        let _ = self.0.busy.set(busy);
    }
    pub fn catalog(&self) -> &Catalog {
        &self.0.catalog
    }
    /// The registry the agent-login flows publish their callback ports to.
    pub fn callback_routes(&self) -> CallbackRoutes {
        self.0.callback_routes.clone()
    }
    /// Forward loopback `port` on this device to login `login_id`'s callback
    /// on `device` until [`Self::close_login_tunnel`] or `lifetime` passes.
    /// Idempotent per login; a port taken here is an error.
    pub async fn open_login_tunnel(
        &self,
        login_id: &str,
        device: &str,
        port: u16,
        lifetime: Duration,
    ) -> anyhow::Result<()> {
        if self.0.callback_tunnels.contains(login_id) {
            return Ok(());
        }
        let peers = self.0.peers.get().cloned().ok_or_else(|| {
            anyhow::anyhow!(
                "Direct connections to other devices aren't available yet — try again in a moment."
            )
        })?;
        let device = device.to_owned();
        let service = login::service_id(login_id);
        let open: login::Opener = Arc::new(move || {
            let (peers, device, service) = (peers.clone(), device.clone(), service.clone());
            Box::pin(
                async move { Ok(Box::new(peers.open(&device, &service, false).await?) as BoxIo) },
            )
        });
        let forwarder = CallbackForwarder::bind(port, lifetime, open).await?;
        self.0.callback_tunnels.insert(login_id, forwarder);
        Ok(())
    }
    pub fn close_login_tunnel(&self, login_id: &str) {
        self.0.callback_tunnels.close(login_id);
    }
    pub async fn start(&self, projects: Projects, signaling: Option<signaling::Config>) {
        if self.0.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let connector = Arc::new(LocalConnector(
            self.0.catalog.clone(),
            self.0.callback_routes.clone(),
        ));
        let local = mux::local(connector.clone(), self.0.stop.child_token());
        let (peers, output) = Peers::new(
            self.0.catalog.device_id().into(),
            connector,
            self.0.stop.child_token(),
        );
        let _ = self.0.peers.set(peers.clone());
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
        let mut tasks = self.0.tasks.lock().unwrap();
        if discovery::SUPPORTED {
            let scanner = Scanner {
                catalog: self.0.catalog.clone(),
                projects,
                root: std::process::id(),
                blocking: Arc::default(),
                memory: Arc::default(),
            };
            let busy = self.0.busy.get().cloned();
            tasks.push(tokio::spawn(discovery_loop(
                move || scanner.clone().pass(),
                move || busy.as_ref().is_some_and(|busy| busy()),
                self.0.discovery_wake.clone(),
                self.0.stop.clone(),
            )));
        }
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
struct LocalConnector(Catalog, CallbackRoutes);
#[async_trait::async_trait]
impl Connector for LocalConnector {
    async fn connect(&self, id: &str) -> anyhow::Result<BoxIo> {
        self.connect_from(None, id).await
    }
    async fn connect_from(&self, peer: Option<&str>, id: &str) -> anyhow::Result<BoxIo> {
        // A sign-in callback: only the registered port, only for the device
        // that started the login, only while it runs.
        if let Some(port) = self.1.target(peer, id) {
            return CallbackRoutes::connect(port?).await;
        }
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
    use std::sync::atomic::AtomicUsize;

    struct Loop {
        passes: Arc<AtomicUsize>,
        live: Arc<AtomicBool>,
        busy: Arc<AtomicBool>,
        wake: Arc<Notify>,
        stop: CancellationToken,
    }
    fn spawn_loop() -> Loop {
        let passes = Arc::new(AtomicUsize::new(0));
        let live = Arc::new(AtomicBool::new(false));
        let busy = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(Notify::new());
        let stop = CancellationToken::new();
        let (p, l, b) = (passes.clone(), live.clone(), busy.clone());
        tokio::spawn(discovery_loop(
            move || {
                p.fetch_add(1, Ordering::SeqCst);
                let live = l.load(Ordering::SeqCst);
                async move { live }
            },
            move || b.load(Ordering::SeqCst),
            wake.clone(),
            stop.clone(),
        ));
        Loop {
            passes,
            live,
            busy,
            wake,
            stop,
        }
    }
    impl Loop {
        fn passes(&self) -> usize {
            self.passes.load(Ordering::SeqCst)
        }
    }
    /// Lets the loop run, advancing the paused clock by `by`.
    async fn elapse(by: Duration) {
        tokio::time::sleep(by).await;
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn no_pass_and_no_timer_while_nothing_runs() {
        let looped = spawn_loop();
        elapse(Duration::ZERO).await;
        assert_eq!(looped.passes(), 1, "one pass at start");
        elapse(Duration::from_secs(24 * 3600)).await;
        assert_eq!(looped.passes(), 1, "parked: a day passes without a pass");
        looped.stop.cancel();
    }

    #[tokio::test(start_paused = true)]
    async fn passes_resume_on_spawn_and_stop_after_the_last_child() {
        let looped = spawn_loop();
        elapse(Duration::ZERO).await;
        // A run starts: its process appears and the engine wakes discovery.
        looped.live.store(true, Ordering::SeqCst);
        looped.wake.notify_one();
        elapse(Duration::ZERO).await;
        assert_eq!(looped.passes(), 2, "woken immediately");
        elapse(SCAN_INTERVAL * 5).await;
        assert_eq!(looped.passes(), 7, "every interval while children live");
        // The last child exits: one more pass sees that (and withdraws its
        // servers), then the loop parks.
        looped.live.store(false, Ordering::SeqCst);
        elapse(SCAN_INTERVAL).await;
        assert_eq!(looped.passes(), 8);
        elapse(Duration::from_secs(3600)).await;
        assert_eq!(looped.passes(), 8, "parked again");
        looped.stop.cancel();
    }

    #[tokio::test(start_paused = true)]
    async fn a_busy_engine_keeps_polling_before_its_process_exists() {
        let looped = spawn_loop();
        elapse(Duration::ZERO).await;
        // A run was dispatched; its harness has not spawned yet.
        looped.busy.store(true, Ordering::SeqCst);
        looped.wake.notify_one();
        elapse(SCAN_INTERVAL * 3).await;
        assert_eq!(looped.passes(), 5);
        looped.busy.store(false, Ordering::SeqCst);
        elapse(SCAN_INTERVAL * 10).await;
        assert_eq!(looped.passes(), 6);
        looped.stop.cancel();
    }

    #[cfg(unix)]
    #[test]
    fn project_roots_resolve_once_and_follow_the_chat_list() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        let nested = app.join("web");
        std::fs::create_dir_all(&nested).unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&app, &link).unwrap();
        let missing = temp.path().join("missing");
        let mut roots = ProjectRoots::default();
        let t0 = Instant::now();
        let resolved = roots.resolve(vec![link.clone(), nested.clone(), missing.clone()], t0);
        let app = app.canonicalize().unwrap();
        assert_eq!(resolved, [nested.canonicalize().unwrap(), app.clone()]);
        // Cached: a later pass does not touch the filesystem for known folders.
        std::fs::remove_file(&link).unwrap();
        std::fs::create_dir(&missing).unwrap();
        let resolved = roots.resolve(vec![link.clone(), missing.clone()], t0);
        assert_eq!(resolved, [app], "unresolved folders wait out their retry");
        let later = t0 + RETRY_UNRESOLVED_ROOT;
        let resolved = roots.resolve(vec![link.clone(), missing.clone()], later);
        assert_eq!(resolved.len(), 2);
        // Folders no longer listed (archived chats) are forgotten.
        assert!(roots.resolve(Vec::new(), later).is_empty());
        assert!(roots.resolved.is_empty());
    }

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
