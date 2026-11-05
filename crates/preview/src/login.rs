//! Agent sign-in callbacks across devices. A login the user starts from
//! device A but runs on device B redirects A's browser to
//! `localhost:<port>`, while the CLI waiting for that redirect listens on B's
//! loopback. A binds the same port on its own loopback for the life of the
//! login and pipes each connection over the authenticated P2P mux to B, whose
//! connector dials the one registered port — nothing else.
//!
//! B's routes are keyed by login id, bound to the device that started the
//! login, expire with the login, and are removed the moment it ends; A's
//! forwarder is dropped at the same points. A port already taken on A fails
//! the login up front with a clear message rather than racing the browser.
use crate::mux::BoxIo;
use futures::future::BoxFuture;
use std::{
    collections::HashMap,
    net::{Ipv4Addr, Ipv6Addr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{net::TcpListener, sync::Notify};
use tokio_util::sync::CancellationToken;

/// Mux service ids of sign-in callbacks: the prefix, then the login id.
pub const SERVICE_PREFIX: &str = "agent-login:";
/// Concurrent browser connections one forwarder carries (a redirect plus a
/// favicon or retry — never a page's worth of assets).
const MAX_CONNECTIONS: usize = 8;

pub fn service_id(login_id: &str) -> String {
    format!("{SERVICE_PREFIX}{login_id}")
}

struct Route {
    port: u16,
    peer: String,
    expires: Instant,
}

/// The running device's side: which login callbacks a peer may reach.
#[derive(Clone, Default)]
pub struct CallbackRoutes {
    routes: Arc<Mutex<HashMap<String, Route>>>,
    /// Told of every new route, so signaling can rejoin as a sign-in.
    registered: Arc<Notify>,
}

impl CallbackRoutes {
    /// Routes that notify `registered` whenever one is added.
    pub(crate) fn notifying(registered: Arc<Notify>) -> Self {
        Self {
            routes: Default::default(),
            registered,
        }
    }

    /// Let `peer` reach loopback `port` for `login_id` until `ttl` passes or
    /// [`Self::remove`] — whichever comes first.
    pub fn register(&self, login_id: &str, port: u16, peer: &str, ttl: Duration) {
        self.routes.lock().unwrap().insert(
            login_id.to_owned(),
            Route {
                port,
                peer: peer.to_owned(),
                expires: Instant::now() + ttl,
            },
        );
        self.registered.notify_one();
    }

    pub fn remove(&self, login_id: &str) {
        self.routes.lock().unwrap().remove(login_id);
    }

    pub fn is_registered(&self, login_id: &str) -> bool {
        self.routes.lock().unwrap().contains_key(login_id)
    }

    /// Whether any login is waiting for a peer's callback right now.
    pub(crate) fn any_live(&self) -> bool {
        let now = Instant::now();
        self.routes
            .lock()
            .unwrap()
            .values()
            .any(|route| route.expires > now)
    }

    /// The loopback port of a live login `peer` started — for a device that
    /// relays the browser's callback request itself rather than piping a
    /// connection (the phone has no P2P mux).
    pub fn port_for(&self, login_id: &str, peer: &str) -> anyhow::Result<u16> {
        match self.target(Some(peer), &service_id(login_id)) {
            Some(result) => result,
            None => Err(anyhow::anyhow!("no sign-in is waiting for this callback")),
        }
    }

    /// Where a peer's `service` open goes: `None` when it isn't a sign-in
    /// callback at all, an error unless it names a live login this `peer`
    /// started. Local (same-device) opens carry no peer and are refused.
    pub(crate) fn target(&self, peer: Option<&str>, service: &str) -> Option<anyhow::Result<u16>> {
        let login_id = service.strip_prefix(SERVICE_PREFIX)?;
        let mut routes = self.routes.lock().unwrap();
        let now = Instant::now();
        routes.retain(|_, route| route.expires > now);
        Some(match (routes.get(login_id), peer) {
            (Some(route), Some(peer)) if route.peer == peer => Ok(route.port),
            (Some(_), _) => Err(anyhow::anyhow!(
                "this sign-in callback belongs to another device"
            )),
            (None, _) => Err(anyhow::anyhow!("no sign-in is waiting for this callback")),
        })
    }

    /// Dial a login's loopback callback. CLIs bind either loopback family.
    pub(crate) async fn connect(port: u16) -> anyhow::Result<BoxIo> {
        match tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await {
            Ok(socket) => Ok(Box::new(socket)),
            Err(v4) => match tokio::net::TcpStream::connect((Ipv6Addr::LOCALHOST, port)).await {
                Ok(socket) => Ok(Box::new(socket)),
                Err(_) => Err(v4.into()),
            },
        }
    }
}

/// Opens one stream to the remote login's callback (a mux stream in
/// production, anything bidirectional in tests).
pub type Opener = Arc<dyn Fn() -> BoxFuture<'static, anyhow::Result<BoxIo>> + Send + Sync>;

/// The requesting device's side: a loopback listener on the login's port that
/// pipes every connection through [`Opener`]. Dropping it stops listening and
/// cuts any connection still open.
pub struct CallbackForwarder {
    port: u16,
    stop: CancellationToken,
}

impl Drop for CallbackForwarder {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

impl CallbackForwarder {
    /// Listen on `port` (both loopback families, like the CLIs' redirect
    /// targets) for at most `lifetime`. A port another app owns is an error —
    /// its browser would otherwise land on that app instead.
    pub async fn bind(port: u16, lifetime: Duration, open: Opener) -> anyhow::Result<Self> {
        let in_use = |port: u16| {
            anyhow::anyhow!(
                "Port {port} is already in use on this device, so the sign-in can't finish \
                 here. Quit whatever is using it and try again."
            )
        };
        let v4 = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::AddrInUse => in_use(port),
                _ => anyhow::anyhow!("Could not listen on port {port} for the sign-in: {error}"),
            })?;
        let port = v4.local_addr()?.port();
        let mut listeners = vec![v4];
        match TcpListener::bind((Ipv6Addr::LOCALHOST, port)).await {
            Ok(v6) => listeners.push(v6),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                return Err(in_use(port));
            }
            // No IPv6 loopback on this host: `localhost` can't resolve there.
            Err(_) => {}
        }
        let stop = CancellationToken::new();
        let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
        for listener in listeners {
            let stop = stop.clone();
            let open = open.clone();
            let slots = slots.clone();
            tokio::spawn(async move {
                let serve = async {
                    loop {
                        let Ok((mut browser, _)) = listener.accept().await else {
                            break;
                        };
                        let Ok(permit) = slots.clone().try_acquire_owned() else {
                            continue;
                        };
                        let open = open.clone();
                        let stop = stop.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            tokio::select! {
                                _ = stop.cancelled() => {}
                                _ = async {
                                    match open().await {
                                        Ok(mut remote) => {
                                            let _ = tokio::io::copy_bidirectional(&mut browser, &mut remote).await;
                                        }
                                        Err(error) => tracing::debug!(%error, "sign-in callback forward refused"),
                                    }
                                } => {}
                            }
                        });
                    }
                };
                tokio::select! {
                    _ = stop.cancelled() => {}
                    _ = tokio::time::sleep(lifetime) => stop.cancel(),
                    _ = serve => {}
                }
            });
        }
        Ok(Self { port, stop })
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

/// Live forwarders by login id — at most one per login.
#[derive(Clone, Default)]
pub struct CallbackTunnels(Arc<Mutex<HashMap<String, CallbackForwarder>>>);

impl CallbackTunnels {
    pub fn contains(&self, login_id: &str) -> bool {
        self.0.lock().unwrap().contains_key(login_id)
    }

    pub fn insert(&self, login_id: &str, forwarder: CallbackForwarder) {
        self.0
            .lock()
            .unwrap()
            .insert(login_id.to_owned(), forwarder);
    }

    /// Stop forwarding `login_id` (a no-op when none runs).
    pub fn close(&self, login_id: &str) {
        self.0.lock().unwrap().remove(login_id);
    }

    /// Whether any forwarder is still listening.
    pub(crate) fn any_live(&self) -> bool {
        self.0
            .lock()
            .unwrap()
            .values()
            .any(|forwarder| !forwarder.stop.is_cancelled())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::{Connector, Mux, PeerScoped, SocketTransport};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// The running device's connector, reduced to its sign-in routes.
    struct Routes(CallbackRoutes);
    #[async_trait::async_trait]
    impl Connector for Routes {
        async fn connect(&self, service: &str) -> anyhow::Result<BoxIo> {
            self.connect_from(None, service).await
        }
        async fn connect_from(&self, peer: Option<&str>, service: &str) -> anyhow::Result<BoxIo> {
            let port = self
                .0
                .target(peer, service)
                .unwrap_or_else(|| Err(anyhow::anyhow!("not a sign-in callback")))?;
            CallbackRoutes::connect(port).await
        }
    }

    /// A CLI's loopback callback: answers each connection with its port.
    async fn callback_server() -> (u16, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut request = [0u8; 64];
                    let n = socket.read(&mut request).await.unwrap_or(0);
                    let reply = format!("{port}:{}", String::from_utf8_lossy(&request[..n]));
                    let _ = socket.write_all(reply.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        (port, task)
    }

    /// A mux pair standing in for the P2P link: `requester` opens streams that
    /// the running device serves as `peer`.
    fn link(routes: &CallbackRoutes, peer: &str, stop: &CancellationToken) -> Mux {
        let (a, b) = tokio::net::UnixStream::pair().unwrap();
        let (ar, aw) = tokio::io::split(a);
        let (br, bw) = tokio::io::split(b);
        let connector: Arc<dyn Connector> = Arc::new(Routes(routes.clone()));
        Mux::start(
            Arc::new(SocketTransport::new(br, bw)),
            Arc::new(PeerScoped::new(peer, connector.clone())),
            false,
            stop.child_token(),
        );
        Mux::start(
            Arc::new(SocketTransport::new(ar, aw)),
            connector,
            true,
            stop.child_token(),
        )
    }

    fn opener(mux: &Mux, login_id: &str) -> Opener {
        let mux = mux.clone();
        let service = service_id(login_id);
        Arc::new(move || {
            let mux = mux.clone();
            let service = service.clone();
            Box::pin(async move { Ok(Box::new(mux.open(&service, false).await?) as BoxIo) })
        })
    }

    async fn round_trip(port: u16) -> std::io::Result<String> {
        let mut socket = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;
        socket.write_all(b"GET /callback").await?;
        let mut reply = String::new();
        tokio::time::timeout(Duration::from_secs(10), socket.read_to_string(&mut reply))
            .await
            .map_err(|_| std::io::Error::other("stalled"))??;
        Ok(reply)
    }

    /// The browser gets nothing back: the connection is closed or reset
    /// without a byte of the callback's reply.
    async fn refused(port: u16) -> bool {
        round_trip(port)
            .await
            .map_or(true, |reply| reply.is_empty())
    }

    #[tokio::test]
    async fn forwards_the_callback_to_the_registered_port_for_the_starting_device() {
        let stop = CancellationToken::new();
        let (callback, _server) = callback_server().await;
        let (_decoy, _decoy_server) = callback_server().await;
        let routes = CallbackRoutes::default();
        routes.register("login-1", callback, "device-a", Duration::from_secs(60));
        let mux = link(&routes, "device-a", &stop);
        let forwarder =
            CallbackForwarder::bind(0, Duration::from_secs(60), opener(&mux, "login-1"))
                .await
                .unwrap();
        let reply = round_trip(forwarder.port()).await.unwrap();
        assert_eq!(reply, format!("{callback}:GET /callback"));
        stop.cancel();
    }

    #[tokio::test]
    async fn another_device_or_an_unknown_login_is_refused() {
        let stop = CancellationToken::new();
        let (callback, _server) = callback_server().await;
        let routes = CallbackRoutes::default();
        routes.register("login-1", callback, "device-a", Duration::from_secs(60));
        // The link authenticates as device-b, which did not start login-1.
        let other = link(&routes, "device-b", &stop);
        let forwarder =
            CallbackForwarder::bind(0, Duration::from_secs(60), opener(&other, "login-1"))
                .await
                .unwrap();
        assert!(refused(forwarder.port()).await);
        // Same device, but a login it never registered.
        let mux = link(&routes, "device-a", &stop);
        let forwarder =
            CallbackForwarder::bind(0, Duration::from_secs(60), opener(&mux, "login-2"))
                .await
                .unwrap();
        assert!(refused(forwarder.port()).await);
        // Local opens carry no peer at all.
        assert!(
            routes
                .target(None, &service_id("login-1"))
                .unwrap()
                .is_err()
        );
        assert!(routes.target(Some("device-a"), "preview-service").is_none());
        stop.cancel();
    }

    #[tokio::test]
    async fn the_route_and_the_forwarder_end_with_the_login() {
        let stop = CancellationToken::new();
        let (callback, _server) = callback_server().await;
        let routes = CallbackRoutes::default();
        routes.register("login-1", callback, "device-a", Duration::from_secs(60));
        let mux = link(&routes, "device-a", &stop);
        let tunnels = CallbackTunnels::default();
        let forwarder =
            CallbackForwarder::bind(0, Duration::from_secs(60), opener(&mux, "login-1"))
                .await
                .unwrap();
        let port = forwarder.port();
        tunnels.insert("login-1", forwarder);
        assert!(
            round_trip(port)
                .await
                .unwrap()
                .starts_with(&callback.to_string())
        );
        // The login finished on the running device: its route is gone.
        routes.remove("login-1");
        assert!(refused(port).await);
        // …and the requester stops listening on the port.
        tunnels.close("login-1");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            round_trip(port).await.is_err(),
            "port {port} still forwards"
        );
        // An expired route refuses on its own, before anyone removes it.
        routes.register("login-3", callback, "device-a", Duration::ZERO);
        assert!(
            routes
                .target(Some("device-a"), &service_id("login-3"))
                .unwrap()
                .is_err()
        );
        stop.cancel();
    }

    #[tokio::test]
    async fn a_busy_local_port_is_a_clear_error() {
        let taken = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = taken.local_addr().unwrap().port();
        let open: Opener = Arc::new(|| Box::pin(async { anyhow::bail!("never dialed") }));
        let error = CallbackForwarder::bind(port, Duration::from_secs(60), open)
            .await
            .err()
            .expect("a taken port must not forward");
        let message = error.to_string();
        assert!(
            message.contains(&format!("Port {port} is already in use")),
            "{message}"
        );
    }

    #[tokio::test]
    async fn a_sign_in_counts_as_live_only_while_it_can_be_reached() {
        let registered = Arc::new(Notify::new());
        let routes = CallbackRoutes::notifying(registered.clone());
        assert!(!routes.any_live());
        routes.register("login-1", 1234, "device-a", Duration::from_secs(60));
        assert!(routes.any_live());
        // Registering woke signaling (the permit waits for it).
        tokio::time::timeout(Duration::from_secs(1), registered.notified())
            .await
            .expect("a new route wakes signaling");
        routes.remove("login-1");
        routes.register("login-2", 1234, "device-a", Duration::ZERO);
        assert!(!routes.any_live(), "an expired route is no sign-in");

        let tunnels = CallbackTunnels::default();
        let open: Opener = Arc::new(|| Box::pin(async { anyhow::bail!("never dialed") }));
        let forwarder = CallbackForwarder::bind(0, Duration::from_millis(50), open)
            .await
            .unwrap();
        tunnels.insert("login-1", forwarder);
        assert!(tunnels.any_live());
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !tunnels.any_live(),
            "a forwarder past its lifetime is no sign-in"
        );
    }

    #[tokio::test]
    async fn the_forwarder_stops_after_its_lifetime() {
        let open: Opener = Arc::new(|| Box::pin(async { anyhow::bail!("never dialed") }));
        let forwarder = CallbackForwarder::bind(0, Duration::from_millis(50), open)
            .await
            .unwrap();
        let port = forwarder.port();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
                .await
                .is_err(),
            "port {port} outlived the login"
        );
    }
}
