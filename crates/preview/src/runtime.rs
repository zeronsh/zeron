//! The WebRTC runtime for direct links: webrtc's own tokio runtime, except
//! that its UDP sockets may fragment.
//!
//! Its sockets go through quinn-udp, which forbids fragmentation (QUIC sizes
//! its own packets). A full-size SCTP packet is 1228 bytes before DTLS and
//! the IP/UDP headers — ~1293 on the wire — while a tunnel such as
//! Tailscale's carries 1280. Once ICE picks a pair over such a link, every
//! full packet fails to send (EMSGSIZE) and is retried forever: the ordered
//! channel stalls behind it, and nothing on the link gets through. Letting
//! the kernel fragment those packets restores what a plain UDP socket does.
use std::{fmt, future::Future, io, net::SocketAddr, pin::Pin, sync::Arc, time::Duration};
use webrtc::runtime::{
    AsyncInterval, AsyncTcpListener, AsyncTcpStream, AsyncUdpSocket, JoinHandle, Runtime,
    TokioRuntime,
};

pub(crate) fn runtime() -> Arc<dyn Runtime> {
    Arc::new(Fragmenting(TokioRuntime))
}

struct Fragmenting(TokioRuntime);

impl fmt::Debug for Fragmenting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Fragmenting(TokioRuntime)")
    }
}

impl Runtime for Fragmenting {
    #[track_caller]
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + Send>>) -> Box<dyn JoinHandle> {
        self.0.spawn(future)
    }
    fn spawn_reactor(
        &self,
        reactor_pool_size: usize,
        future: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> Box<dyn JoinHandle> {
        self.0.spawn_reactor(reactor_pool_size, future)
    }
    fn wrap_udp_socket(&self, socket: std::net::UdpSocket) -> io::Result<Arc<dyn AsyncUdpSocket>> {
        // Options belong to the socket, not the descriptor: a duplicate lets
        // us undo quinn-udp's after it has set them.
        let duplicate = socket.try_clone()?;
        let wrapped = self.0.wrap_udp_socket(socket)?;
        allow_fragmentation(&duplicate);
        Ok(wrapped)
    }
    fn wrap_tcp_listener(
        &self,
        listener: std::net::TcpListener,
    ) -> io::Result<Arc<dyn AsyncTcpListener>> {
        self.0.wrap_tcp_listener(listener)
    }
    fn connect_tcp<'a>(
        &'a self,
        remote_addr: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = io::Result<Arc<dyn AsyncTcpStream>>> + Send + 'a>> {
        self.0.connect_tcp(remote_addr)
    }
    fn resolve_host<'a>(
        &'a self,
        host: &'a str,
    ) -> Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'a>> {
        self.0.resolve_host(host)
    }
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        self.0.sleep(duration)
    }
    fn interval(&self, period: Duration) -> Box<dyn AsyncInterval> {
        self.0.interval(period)
    }
    fn block_on(&self, future: Pin<Box<dyn Future<Output = ()> + '_>>) {
        self.0.block_on(future)
    }
    fn yield_now(&self) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        self.0.yield_now()
    }
    fn name(&self) -> &'static str {
        self.0.name()
    }
}

/// Clear what quinn-udp set to forbid fragmentation. Best effort: an option
/// a socket's family doesn't have just fails.
#[cfg(unix)]
fn allow_fragmentation(socket: &std::net::UdpSocket) {
    use std::os::fd::AsRawFd;
    let fd = socket.as_raw_fd();
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        set_option(
            fd,
            libc::IPPROTO_IP,
            libc::IP_MTU_DISCOVER,
            libc::IP_PMTUDISC_DONT,
        );
        set_option(
            fd,
            libc::IPPROTO_IPV6,
            libc::IPV6_MTU_DISCOVER,
            libc::IPV6_PMTUDISC_DONT,
        );
    }
    #[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
    set_option(fd, libc::IPPROTO_IP, libc::IP_DONTFRAG, 0);
    set_option(fd, libc::IPPROTO_IPV6, libc::IPV6_DONTFRAG, 0);
}

#[cfg(unix)]
fn set_option(fd: std::os::fd::RawFd, level: libc::c_int, name: libc::c_int, value: libc::c_int) {
    // SAFETY: `fd` is an open socket for the call, and `value` outlives it.
    unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            &value as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
    }
}

#[cfg(windows)]
fn allow_fragmentation(socket: &std::net::UdpSocket) {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock;
    let socket = socket.as_raw_socket() as WinSock::SOCKET;
    for (level, name) in [
        (WinSock::IPPROTO_IP, WinSock::IP_DONTFRAGMENT),
        (WinSock::IPPROTO_IPV6, WinSock::IPV6_DONTFRAG),
    ] {
        let off: u32 = 0;
        // SAFETY: `socket` is open for the call, and `off` outlives it.
        unsafe {
            WinSock::setsockopt(
                socket,
                level,
                name,
                &off as *const u32 as _,
                std::mem::size_of_val(&off) as i32,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether the kernel would refuse to fragment this socket's datagrams.
    #[cfg(unix)]
    fn forbids_fragmentation(socket: &std::net::UdpSocket) -> bool {
        use std::os::fd::AsRawFd;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let (name, forbidding) = (libc::IP_MTU_DISCOVER, libc::IP_PMTUDISC_PROBE);
        #[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
        let (name, forbidding) = (libc::IP_DONTFRAG, 1);
        let mut value: libc::c_int = -1;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: `socket` is open, and `value`/`len` outlive the call.
        let rc = unsafe {
            libc::getsockopt(
                socket.as_raw_fd(),
                libc::IPPROTO_IP,
                name,
                &mut value as *mut libc::c_int as *mut libc::c_void,
                &mut len,
            )
        };
        assert_eq!(rc, 0, "{}", io::Error::last_os_error());
        value == forbidding
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn direct_link_sockets_may_fragment() {
        let wrap = |runtime: &dyn Runtime| {
            let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            let duplicate = socket.try_clone().unwrap();
            let _wrapped = runtime.wrap_udp_socket(socket).unwrap();
            forbids_fragmentation(&duplicate)
        };
        // The stock runtime forbids it — the stall this module exists for…
        assert!(wrap(&TokioRuntime));
        // …and ours lets the kernel split an oversized packet instead.
        assert!(!wrap(runtime().as_ref()));
    }
}
