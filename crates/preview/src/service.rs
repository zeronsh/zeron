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
    collections::{HashMap, HashSet},
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
/// How often discovery scans while something Zeron started is alive.
const SCAN_CADENCE: Duration = Duration::from_secs(2);
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
    /// Redials a refused signaling connection when a sign-in starts.
    signaling_wake: Arc<tokio::sync::Notify>,
    peers: std::sync::OnceLock<Peers>,
    /// Engine-provided stream services peers may open, by id prefix.
    streams: StreamServices,
    /// Wakes a parked discovery scan ([`PreviewService::rescan`]).
    scan_wake: Arc<tokio::sync::Notify>,
}
/// Stream services beyond previews (terminals), keyed by service id prefix.
/// Each handler sees the authenticated peer that opened the stream and
/// decides itself whether to serve it.
type StreamServices = Arc<Mutex<Vec<(String, Arc<dyn Connector>)>>>;

/// What discovery serves, re-read every scan.
#[derive(Clone, Default)]
pub struct Scope {
    /// Every chat this device hosts, with its checkout. A listener counts only
    /// when one of these chats' agents or terminals started it; it is listed
    /// under the deepest checkout containing its working directory, else under
    /// the checkout of the chat that started it.
    pub chats: Vec<(String, PathBuf)>,
    /// Terminal id → chat, for the engine's terminals (project actions run in
    /// one), including ones since closed.
    pub terminals: HashMap<String, String>,
    /// Ports agent CLIs serve their own protocol on (`opencode serve`).
    pub reserved_ports: HashSet<u16>,
    /// A run or terminal here is live now. Discovery keeps its cadence
    /// while it is, even before the agent's own process exists.
    pub live: bool,
}
pub type Projects = Arc<dyn Fn() -> Scope + Send + Sync>;

/// This scan's candidates (`None` when the process table could not be read),
/// each with the checkout it is listed under, and whether anything that
/// could start one is alive: a live run or terminal, or a process one of
/// them started ([`discovery::Owned`]). Only then is the next scan due.
fn candidates(
    scope: Scope,
    origins: &mut discovery::Origins,
) -> Option<(Vec<(PathBuf, discovery::Listener)>, bool)> {
    let Scope {
        chats,
        terminals,
        reserved_ports,
        live,
    } = scope;
    let mut canonical = HashMap::new();
    let chats: HashMap<String, PathBuf> = chats
        .into_iter()
        .filter_map(|(chat, cwd)| {
            let cwd = canonical
                .entry(cwd.clone())
                .or_insert_with(|| cwd.canonicalize().ok())
                .clone()?;
            Some((chat, cwd))
        })
        .collect();
    let mut roots: Vec<PathBuf> = chats
        .values()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if roots.is_empty() {
        return Some((Vec::new(), live));
    }
    roots.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    // The origins a listed server can have (see the filter below).
    let accept = |origin: &str| match origin.split_once(':') {
        Some(("chat", chat)) => chats.contains_key(chat),
        Some(("terminal", terminal)) => terminals.get(terminal).is_some_and(|chat| chats.contains_key(chat)),
        _ => false,
    };
    let owned = discovery::owned(origins, &accept)?;
    let active = live || !owned.is_empty();
    let listeners = discovery::owned_listeners(origins, &owned)?;
    Some((
        listeners
            .into_iter()
            .filter(|l| {
                l.zeron_owned
                    && l.address.port() != zeron_proto::PREVIEW_PROXY_PORT
                    && !reserved_ports.contains(&l.address.port())
                    && l.pid != std::process::id()
                    && !l.is_authentication_command()
                    && !l.signing_in
            })
            .filter_map(|mut listener| {
                // A chat this device no longer hosts (or another engine's,
                // inherited by a nested one) owns nothing here.
                let chat = match listener.origin.as_deref()?.split_once(':')? {
                    ("chat", chat) => chat.to_owned(),
                    ("terminal", terminal) => terminals.get(terminal)?.clone(),
                    _ => return None,
                };
                let chat_root = chats.get(&chat)?;
                listener.chat_id = Some(chat);
                let root = roots
                    .iter()
                    .find(|root| listener.belongs_to(root))
                    .unwrap_or(chat_root)
                    .clone();
                Some((root, listener))
            })
            .collect(),
        active,
    ))
}
impl PreviewService {
    pub fn new(file: PathBuf, device_id: String, device_name: String) -> anyhow::Result<Self> {
        let signaling_wake = Arc::new(tokio::sync::Notify::new());
        Ok(Self(Arc::new(Inner {
            scan_wake: Arc::new(tokio::sync::Notify::new()),
            catalog: Catalog::open(file, device_id, device_name)?,
            stop: CancellationToken::new(),
            started: AtomicBool::new(false),
            tasks: Mutex::new(Vec::new()),
            callback_routes: CallbackRoutes::notifying(signaling_wake.clone()),
            callback_tunnels: CallbackTunnels::default(),
            signaling_wake,
            peers: std::sync::OnceLock::new(),
            streams: StreamServices::default(),
        })))
    }
    /// Serve stream ids starting with `prefix` from `handler` — to peers and
    /// to this device alike. Registering a prefix again replaces its handler.
    pub fn serve_streams(&self, prefix: &str, handler: Arc<dyn Connector>) {
        let mut streams = self.0.streams.lock().unwrap();
        streams.retain(|(existing, _)| existing != prefix);
        streams.push((prefix.to_owned(), handler));
    }
    /// The open direct connection to `device`, if there is one right now.
    pub async fn peer_ready(&self, device: &str) -> bool {
        match self.0.peers.get() {
            Some(peers) => peers.ready(device).await.is_some(),
            None => false,
        }
    }
    /// Start pairing with `device` in the background, so a stream opened
    /// shortly after finds the direct connection already up.
    pub fn warm_peer(&self, device: &str) {
        let Some(peers) = self.0.peers.get().cloned() else {
            return;
        };
        let device = device.to_owned();
        tokio::spawn(async move {
            if let Err(error) = peers.connect(&device).await {
                tracing::debug!(%error, "direct connection warm-up failed");
            }
        });
    }
    /// Open stream `service` on `device` directly, pairing first if needed
    /// (bounded: pairing gives up after 30s).
    pub async fn open_peer_stream(
        &self,
        device: &str,
        service: &str,
    ) -> anyhow::Result<mux::Stream> {
        let peers = self.0.peers.get().cloned().ok_or_else(|| {
            anyhow::anyhow!("Direct connections to other devices aren't available")
        })?;
        peers.open(device, service, false).await
    }
    pub fn catalog(&self) -> &Catalog {
        &self.0.catalog
    }
    /// Scan for development servers now: a run or terminal here started or
    /// changed state. Discovery parks while nothing Zeron started is alive,
    /// and only this wakes it.
    pub fn rescan(&self) {
        self.0.scan_wake.notify_one();
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
        self.0.signaling_wake.notify_one();
        Ok(())
    }
    pub fn close_login_tunnel(&self, login_id: &str) {
        self.0.callback_tunnels.close(login_id);
    }
    /// A provider sign-in's callback rides the direct connection on either
    /// end — the device running the login, or the one forwarding to it — so
    /// signaling joins as a sign-in, past the coordinator's device limit.
    fn signing_in(&self) -> signaling::SigningIn {
        let (routes, tunnels) = (
            self.0.callback_routes.clone(),
            self.0.callback_tunnels.clone(),
        );
        Arc::new(move || routes.any_live() || tunnels.any_live())
    }
    pub async fn start(&self, projects: Projects, signaling: Option<signaling::Config>) {
        if self.0.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let connector = Arc::new(LocalConnector(
            self.0.catalog.clone(),
            self.0.callback_routes.clone(),
            self.0.streams.clone(),
            self.0.scan_wake.clone(),
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
                    binding = serve_proxy(router.clone(), &proxy_stop) => binding,
                };
                match binding {
                    Ok((port, tasks)) => {
                        proxy_catalog.set_proxy_status(port, None);
                        for task in tasks {
                            let _ = task.await;
                        }
                        break;
                    }
                    Err(error) => {
                        tracing::warn!(%error, "local previews could not start; retrying");
                        proxy_catalog.set_proxy_status(
                            zeron_proto::PREVIEW_PROXY_PORT,
                            Some(format!("Local previews could not start: {error}")),
                        )
                    }
                }
                tokio::select! { _ = proxy_stop.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
            }
        });
        self.0.tasks.lock().unwrap().push(listener_task);
        let catalog = self.0.catalog.clone();
        let stop = self.0.stop.clone();
        let wake = self.0.scan_wake.clone();
        // Every 2 s while anything Zeron started here is alive (a run, a
        // terminal, or a process either left running): a server one starts
        // is listed within one cadence, as before. With none alive no
        // listener could be listed, so the scan parks until a run or
        // terminal changes state, or an open preview loses its process.
        let scanner = tokio::spawn(async move {
            let mut memory = ProbeMemory::default();
            let mut origins = discovery::Origins::default();
            loop {
                let memory = &mut memory;
                let origins = &mut origins;
                let scan = async {
                    let projects = projects.clone();
                    let taken = std::mem::take(origins);
                    let scanned = tokio::task::spawn_blocking(move || {
                        let mut origins = taken;
                        (candidates(projects(), &mut origins), origins)
                    })
                    .await;
                    let (candidates, active) = match scanned {
                        Ok((Some(scanned), kept)) => {
                            *origins = kept;
                            scanned
                        }
                        // A failed enumeration says nothing about what runs:
                        // keep the services already listed, and look again.
                        Ok((None, kept)) => {
                            *origins = kept;
                            tracing::debug!(
                                "preview scan could not read processes; keeping the last result"
                            );
                            return true;
                        }
                        Err(error) => {
                            tracing::warn!(%error, "preview scan failed; keeping the last result");
                            return true;
                        }
                    };
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
                    active
                };
                let active = tokio::select! { _ = stop.cancelled() => break, active = scan => active };
                if active {
                    // (A wake meanwhile is served by this cadence.)
                    tokio::select! { _ = stop.cancelled() => break, _ = tokio::time::sleep(SCAN_CADENCE) => {} }
                } else {
                    tokio::select! { _ = stop.cancelled() => break, _ = wake.notified() => {} }
                }
            }
        });
        let mut tasks = self.0.tasks.lock().unwrap();
        tasks.push(scanner);
        if let Some(config) = signaling {
            tasks.push(tokio::spawn(signaling::run_signing_in(
                config,
                self.0.catalog.clone(),
                peers,
                output,
                self.0.stop.child_token(),
                self.0.signaling_wake.clone(),
                self.signing_in(),
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
struct LocalConnector(Catalog, CallbackRoutes, StreamServices, Arc<tokio::sync::Notify>);
#[async_trait::async_trait]
impl Connector for LocalConnector {
    async fn connect(&self, id: &str) -> anyhow::Result<BoxIo> {
        self.connect_from(None, id).await
    }
    async fn connect_from(&self, peer: Option<&str>, id: &str) -> anyhow::Result<BoxIo> {
        let handler = self
            .2
            .lock()
            .unwrap()
            .iter()
            .find(|(prefix, _)| id.starts_with(prefix.as_str()))
            .map(|(_, handler)| handler.clone());
        if let Some(handler) = handler {
            return handler.connect_from(peer, id).await;
        }
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
        if !valid {
            // Rediscovery: the scan may be parked.
            self.3.notify_one();
        }
        anyhow::ensure!(valid, "preview process changed; waiting for rediscovery");
        Ok(Box::new(
            tokio::net::TcpStream::connect(route.listener.address).await?,
        ))
    }
}


/// The local preview proxy, listening: on its usual port, or, with that
/// taken (another Zeron engine on this computer has it: the app's and a
/// development one, say), on one the system picks. Every preview address is
/// built from the port it got (`PreviewSnapshot::proxy_port`), so nothing
/// that opens a preview depends on which.
async fn serve_proxy(
    router: proxy::Router,
    stop: &CancellationToken,
) -> anyhow::Result<(u16, Vec<tokio::task::JoinHandle<()>>)> {
    match proxy::serve(router.clone(), zeron_proto::PREVIEW_PROXY_PORT, stop.child_token()).await {
        Err(error) if address_in_use(&error) => {
            tracing::info!(
                port = zeron_proto::PREVIEW_PROXY_PORT,
                "the preview proxy's port is taken; listening on another"
            );
            proxy::serve(router, 0, stop.child_token()).await
        }
        served => served,
    }
}

fn address_in_use(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::AddrInUse)
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
            zeron_owned: true,
            origin: Some("chat:chat".into()),
            chat_id: Some("chat".into()),
            signing_in: false,
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
    /// A chat's development server is found by the scan of what Zeron
    /// started, and with nothing Zeron started alive the scan finds nothing
    /// to do: the loop parks instead of scanning again.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_chats_server_is_found_and_nothing_alive_parks_the_scan() {
        let folder = tempfile::tempdir().unwrap();
        let chat = format!("idle-test-{}", std::process::id());
        let scope = |live| Scope {
            chats: vec![(chat.clone(), folder.path().to_path_buf())],
            terminals: HashMap::new(),
            reserved_ports: HashSet::new(),
            live,
        };
        let mut origins = discovery::Origins::default();
        let (found, active) = candidates(scope(false), &mut origins).unwrap();
        assert!(found.is_empty());
        assert!(!active, "nothing of this chat runs: nothing to scan for");
        let (_, active) = candidates(scope(true), &mut origins).unwrap();
        assert!(active, "a live run keeps the cadence before its process exists");

        // What an agent of this chat starts: a listener in its folder.
        struct Kill(std::process::Child);
        impl Drop for Kill {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = std::process::Command::new("python3")
            .args([
                "-c",
                "import socket,sys,time\ns=socket.socket()\ns.bind(('127.0.0.1',0))\ns.listen()\nprint(s.getsockname()[1],flush=True)\ntime.sleep(60)",
            ])
            .current_dir(folder.path())
            .env(zeron_proto::ORIGIN_ENV, zeron_proto::chat_origin(&chat))
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let server = Kill(child);
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(stdout), &mut line).unwrap();
        let port: u16 = line.trim().parse().unwrap();
        let (found, active) = candidates(scope(false), &mut origins).unwrap();
        assert!(active, "a process the chat started is alive");
        assert!(
            found.iter().any(|(_, l)| l.address.port() == port
                && l.pid == server.0.id()
                && l.chat_id.as_deref() == Some(chat.as_str())),
            "the chat's server is listed: {:?}",
            found.iter().map(|(_, l)| (l.pid, l.address)).collect::<Vec<_>>()
        );
        drop(server);
        let (found, active) = candidates(scope(false), &mut origins).unwrap();
        assert!(found.is_empty() && !active, "gone with its process");
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
