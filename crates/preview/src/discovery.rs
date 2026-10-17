//! Observe only processes Zeron itself started — agent runs, terminals and
//! everything below them — never the rest of the user's session. HTTP probes
//! run only after project ownership has been established from the process's
//! actual working directory and commands performing authentication have been
//! excluded.
//!
//! On macOS the process tree, working directories and listening sockets come
//! straight from libproc; a pass over a handful of descendants is a few dozen
//! syscalls. (The scanner used to run two `lsof` passes and a `ps -ax` over
//! every process of the user every cycle — ~0.15 s of CPU per lsof.)
use std::{
    collections::{HashMap, VecDeque},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Clone)]
pub struct Listener {
    pub pid: u32,
    pub parent: u32,
    pub cwd: PathBuf,
    pub args: Vec<String>,
    pub started_at: u64,
    pub address: SocketAddr,
    pub zeron_owned: bool,
}

impl Listener {
    pub fn belongs_to(&self, root: &Path) -> bool {
        self.cwd.starts_with(root)
    }

    /// A login CLI may open an HTTP listener in the project's cwd, but that
    /// listener is a browser callback, not a development server. Some (such as
    /// Infisical) abort the login on an unsolicited HEAD, so exclude explicit
    /// authentication commands before connecting at all. Match whole arguments,
    /// not paths, URLs or source strings that happen to mention authentication.
    pub(crate) fn is_authentication_command(&self) -> bool {
        self.args
            .iter()
            .skip(1)
            .take_while(|arg| arg.as_str() != "--")
            .any(|arg| {
                matches!(
                    arg.as_str(),
                    "login"
                        | "auth"
                        | "authenticate"
                        | "signin"
                        | "sign-in"
                        | "sso"
                        | "oauth"
                        | "oauth2"
                )
            })
    }
}

/// Preserve a useful framework label without advertising process arguments,
/// which may contain credentials. Rank frontends above generic HTTP services.
pub fn framework(args: &[String]) -> (&'static str, u8) {
    for (needle, name) in [
        ("vite", "Vite"),
        ("next", "Next.js"),
        ("astro", "Astro"),
        ("miniflare", "Miniflare"),
    ] {
        if args.iter().any(|arg| {
            let arg = arg.to_lowercase();
            Path::new(&arg).file_stem().and_then(|s| s.to_str()) == Some(needle)
                || arg.contains(&format!("/{needle}/"))
                || arg.starts_with(&format!("{needle}-server"))
        }) {
            return (name, if needle == "miniflare" { 80 } else { 100 });
        }
    }
    if args.first().is_some_and(|s| {
        Path::new(s)
            .file_name()
            .is_some_and(|n| n == "node" || n == "bun" || n == "deno")
    }) {
        ("Node HTTP server", 50)
    } else {
        ("HTTP server", 40)
    }
}

/// Port flags are operational settings, never part of a service identity.
pub fn command_identity(args: &[String]) -> String {
    let mut skip = false;
    let python_http = args.iter().any(|arg| arg == "http.server");
    args.iter()
        .filter_map(|arg| {
            if skip {
                skip = false;
                return None;
            }
            if matches!(arg.as_str(), "--port" | "-p" | "--inspect-port") {
                skip = true;
                return None;
            }
            if arg.starts_with("--port=")
                || arg.starts_with("PORT=")
                || arg.starts_with("--inspect-port=")
            {
                return None;
            }
            if python_http && arg.parse::<u16>().is_ok() {
                return None;
            }
            Some(arg.as_str())
        })
        .collect::<Vec<_>>()
        .join("\0")
}

pub async fn is_http(address: SocketAddr) -> bool {
    tokio::time::timeout(Duration::from_millis(800), async move {
        let mut socket = tokio::net::TcpStream::connect(address).await.ok()?;
        socket
            .write_all(
                format!(
                    "HEAD / HTTP/1.1\r\nHost: localhost:{}\r\nConnection: close\r\n\r\n",
                    address.port()
                )
                .as_bytes(),
            )
            .await
            .ok()?;
        let mut prefix = [0; 12];
        socket.read_exact(&mut prefix).await.ok()?;
        let version = &prefix[..9];
        let status = std::str::from_utf8(&prefix[9..])
            .ok()?
            .parse::<u16>()
            .ok()?;
        ((version == b"HTTP/1.1 " || version == b"HTTP/1.0 ") && (100..600).contains(&status))
            .then_some(())
    })
    .await
    .ok()
    .flatten()
    .is_some()
}

/// Whether this platform can enumerate listeners at all. Elsewhere discovery
/// never runs, rather than waking every cycle to find nothing.
pub const SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "linux"));

/// Bound on processes examined in one pass: an agent that forks without
/// limit must not turn discovery into a CPU sink of its own.
const MAX_PROCESSES: usize = 4096;

/// What one pass needs from the OS. Any query may come back empty because
/// the process exited mid-pass or belongs to another user (a `sudo` child);
/// that process then simply drops out of the pass.
pub(crate) trait ProcessTable {
    /// Direct children of `pid`.
    fn children(&mut self, pid: u32) -> Vec<u32>;
    /// `(parent, start time in ms since the epoch)`; `None` once it exited.
    fn info(&mut self, pid: u32) -> Option<(u32, u64)>;
    /// Loopback-reachable addresses of `pid`'s listening TCP sockets.
    fn listening(&mut self, pid: u32) -> Vec<SocketAddr>;
    /// `(cwd, args)` of `pid`.
    fn details(&mut self, pid: u32) -> Option<(PathBuf, Vec<String>)>;
}

#[derive(Debug, Default)]
pub struct Scan {
    pub listeners: Vec<Listener>,
    /// Some process below the root is still alive, so a later pass may find
    /// a server; `false` means there is nothing left to watch.
    pub live: bool,
}

/// The processes below one root (the engine), remembered between passes.
/// A process found below the root stays in scope for as long as it lives
/// (same pid, same start time), even after its parent exits and it is
/// reparented to launchd/init: `npm run dev &` outliving the agent turn or
/// harness that started it is the normal case, not an edge case.
#[derive(Default)]
pub struct Descendants {
    known: HashMap<u32, u64>,
}

impl Descendants {
    /// One pass over the OS process table below `root`.
    pub fn scan(&mut self, root: u32) -> Scan {
        self.scan_with(&mut platform::Table::new(), root)
    }

    pub(crate) fn scan_with(&mut self, table: &mut impl ProcessTable, root: u32) -> Scan {
        let mut seen: HashMap<u32, (u32, u64)> = HashMap::new();
        let mut queue: VecDeque<(u32, Option<u64>)> = table
            .children(root)
            .into_iter()
            .map(|pid| (pid, None))
            .collect();
        let mut known: Vec<_> = self.known.iter().map(|(&p, &s)| (p, Some(s))).collect();
        known.sort_unstable();
        queue.extend(known);
        while let Some((pid, expected)) = queue.pop_front() {
            if pid == root || pid == 0 || seen.contains_key(&pid) || seen.len() >= MAX_PROCESSES {
                continue;
            }
            let Some((parent, started_at)) = table.info(pid) else {
                continue;
            };
            // A remembered pid now naming a different process was reused
            // after ours exited; it is not ours unless it is reached from
            // the root itself.
            if expected.is_some_and(|expected| expected != started_at) {
                continue;
            }
            seen.insert(pid, (parent, started_at));
            queue.extend(table.children(pid).into_iter().map(|child| (child, None)));
        }
        let mut processes: Vec<_> = seen
            .iter()
            .map(|(&pid, &(parent, started_at))| (pid, parent, started_at))
            .collect();
        processes.sort_unstable();
        let mut listeners = Vec::new();
        for (pid, parent, started_at) in processes {
            let mut addresses = table.listening(pid);
            if addresses.is_empty() {
                continue;
            }
            let Some((cwd, args)) = table.details(pid) else {
                continue;
            };
            addresses.sort();
            addresses.dedup();
            for address in addresses {
                listeners.push(Listener {
                    pid,
                    parent,
                    cwd: cwd.clone(),
                    args: args.clone(),
                    started_at,
                    address,
                    zeron_owned: true,
                });
            }
        }
        let live = !seen.is_empty();
        self.known = seen.into_iter().map(|(pid, (_, s))| (pid, s)).collect();
        Scan { listeners, live }
    }

    #[cfg(test)]
    pub(crate) fn pids(&self) -> std::collections::BTreeSet<u32> {
        self.known.keys().copied().collect()
    }
}

/// Whether `pid` is still the process the scanner observed with `started_at`.
/// Cheap enough to run per proxied connection.
pub fn same_process(pid: u32, started_at: u64) -> bool {
    platform::started_at(pid) == Some(started_at)
}

/// Where a socket bound to `ip:port` can be reached over loopback. Wildcard
/// binds answer on both loopback families.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn loopback_addresses(ip: IpAddr, port: u16) -> Vec<SocketAddr> {
    let ip = ip.to_canonical();
    if port == 0 {
        Vec::new()
    } else if ip.is_unspecified() {
        vec![
            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port),
            SocketAddr::new(Ipv6Addr::LOCALHOST.into(), port),
        ]
    } else if ip.is_loopback() {
        vec![SocketAddr::new(ip, port)]
    } else {
        Vec::new()
    }
}

/// `KERN_PROCARGS2`: `argc`, the executable path, NUL padding, then `argc`
/// NUL-terminated arguments (and the environment, which is never read).
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn parse_procargs(buffer: &[u8]) -> Option<Vec<String>> {
    let argc = usize::try_from(i32::from_ne_bytes(buffer.get(..4)?.try_into().ok()?)).ok()?;
    let rest = &buffer[4..];
    let rest = &rest[rest.iter().position(|b| *b == 0)?..];
    let rest = &rest[rest.iter().position(|b| *b != 0).unwrap_or(rest.len())..];
    let argv: Vec<_> = rest
        .split(|b| *b == 0)
        .take(argc)
        .map(String::from_utf8_lossy)
        .collect();
    // Arguments used to come from `ps -o command=` (argv joined by spaces)
    // split on whitespace. Persisted preview identities hash these tokens, so
    // keep that tokenization: a hostname must not change across an upgrade.
    Some(
        argv.join(" ")
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
    )
}

#[cfg(target_os = "macos")]
mod platform {
    //! libproc. Mirrors of `<sys/proc_info.h>` socket structures the libc
    //! crate lacks; their layout is pinned by tests against the SDK's
    //! `sizeof`/`offsetof` values, and every call checks the kernel filled
    //! exactly the expected size before a field is read.
    use super::ProcessTable;
    use std::{
        ffi::{CStr, c_int},
        mem::{size_of, zeroed},
        net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
        path::PathBuf,
        sync::OnceLock,
    };

    const PROC_PIDFDSOCKETINFO: c_int = 3;
    const SOCKINFO_TCP: i32 = 2;
    const TSI_S_LISTEN: i32 = 1;
    const INI_IPV4: u8 = 0x1;

    #[allow(dead_code)]
    #[repr(C)]
    pub(super) struct ProcFileInfo {
        openflags: u32,
        status: u32,
        offset: i64,
        kind: i32,
        guardflags: u32,
    }
    #[allow(dead_code)]
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub(super) struct InV6 {
        hlim: u8,
        cksum: i32,
        ifindex: u16,
        hops: i16,
    }
    #[allow(dead_code)]
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub(super) struct InSockInfo {
        pub(super) fport: i32,
        /// Network byte order.
        pub(super) lport: i32,
        gencnt: u64,
        flags: u32,
        flow: u32,
        pub(super) vflag: u8,
        ttl: u8,
        rfu: u32,
        faddr: [u8; 16],
        /// IPv4 sits in the last four bytes (`in4in6_addr`).
        pub(super) laddr: [u8; 16],
        v4_tos: u8,
        v6: InV6,
    }
    #[allow(dead_code)]
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub(super) struct TcpSockInfo {
        pub(super) ini: InSockInfo,
        pub(super) state: i32,
        timer: [i32; 4],
        mss: i32,
        flags: u32,
        rfu: u32,
        tp: u64,
    }
    /// Sized by its largest member, `un_sockinfo` (528 bytes).
    #[repr(C)]
    pub(super) union SocketProto {
        pub(super) tcp: TcpSockInfo,
        largest: [u64; 66],
    }
    #[allow(dead_code)]
    #[repr(C)]
    pub(super) struct SocketInfo {
        stat: [u64; 17],
        so: u64,
        pcb: u64,
        kind_type: i32,
        protocol: i32,
        pub(super) family: i32,
        shorts: [i16; 7],
        error: u16,
        oobmark: u32,
        rcv: [u32; 6],
        snd: [u32; 6],
        pub(super) kind: i32,
        rfu: u32,
        pub(super) proto: SocketProto,
    }
    #[repr(C)]
    pub(super) struct SocketFdInfo {
        pfi: ProcFileInfo,
        pub(super) psi: SocketInfo,
    }

    pub(crate) struct Table;

    impl Table {
        pub(crate) fn new() -> Self {
            Self
        }
    }

    impl ProcessTable for Table {
        fn children(&mut self, pid: u32) -> Vec<u32> {
            let mut capacity = 64usize;
            loop {
                let mut pids = vec![0 as libc::pid_t; capacity];
                // SAFETY: the buffer holds `capacity` pids; the kernel writes
                // at most that many and returns how many it wrote.
                let count = unsafe {
                    libc::proc_listchildpids(
                        pid as libc::pid_t,
                        pids.as_mut_ptr().cast(),
                        (capacity * size_of::<libc::pid_t>()) as c_int,
                    )
                };
                let Ok(count) = usize::try_from(count) else {
                    return Vec::new();
                };
                // A full buffer may have been truncated: retry larger.
                if count < capacity || capacity >= 1 << 16 {
                    pids.truncate(count.min(capacity));
                    return pids
                        .into_iter()
                        .filter(|pid| *pid > 0)
                        .map(|pid| pid as u32)
                        .collect();
                }
                capacity *= 4;
            }
        }

        fn info(&mut self, pid: u32) -> Option<(u32, u64)> {
            bsd_info(pid)
        }

        fn listening(&mut self, pid: u32) -> Vec<SocketAddr> {
            let entry = size_of::<libc::proc_fdinfo>();
            // SAFETY: a null buffer asks only for the size required.
            let needed = unsafe {
                libc::proc_pidinfo(
                    pid as c_int,
                    libc::PROC_PIDLISTFDS,
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            let Ok(needed) = usize::try_from(needed) else {
                return Vec::new();
            };
            if needed == 0 {
                return Vec::new();
            }
            // Room for descriptors opened between the two calls.
            let capacity = needed / entry + 16;
            // SAFETY: `proc_fdinfo` is plain integers.
            let mut fds: Vec<libc::proc_fdinfo> = vec![unsafe { zeroed() }; capacity];
            // SAFETY: the buffer holds `capacity` entries.
            let written = unsafe {
                libc::proc_pidinfo(
                    pid as c_int,
                    libc::PROC_PIDLISTFDS,
                    0,
                    fds.as_mut_ptr().cast(),
                    (capacity * entry) as c_int,
                )
            };
            let Ok(written) = usize::try_from(written) else {
                return Vec::new();
            };
            fds.truncate((written / entry).min(capacity));
            let mut addresses = Vec::new();
            for fd in fds {
                if fd.proc_fdtype != libc::PROX_FDTYPE_SOCKET as u32 {
                    continue;
                }
                let Some(socket) = socket_info(pid, fd.proc_fd) else {
                    continue;
                };
                if socket.psi.kind != SOCKINFO_TCP {
                    continue;
                }
                // SAFETY: `soi_kind == SOCKINFO_TCP` selects the TCP member.
                let tcp = unsafe { socket.psi.proto.tcp };
                if tcp.state == TSI_S_LISTEN {
                    addresses.extend(listen_addresses(
                        tcp.ini.vflag,
                        tcp.ini.laddr,
                        tcp.ini.lport,
                    ));
                }
            }
            addresses
        }

        fn details(&mut self, pid: u32) -> Option<(PathBuf, Vec<String>)> {
            // Same form lsof-derived cwds had: canonical, so project roots
            // (also canonical) match through /tmp → /private/tmp and the like.
            let cwd = std::fs::canonicalize(cwd(pid)?).ok()?;
            Some((cwd, arguments(pid)?))
        }
    }

    pub(super) fn socket_info(pid: u32, fd: i32) -> Option<SocketFdInfo> {
        // SAFETY: plain integers/unions of integers.
        let mut info: SocketFdInfo = unsafe { zeroed() };
        let size = size_of::<SocketFdInfo>() as c_int;
        // SAFETY: the buffer is exactly `size` bytes.
        let written = unsafe {
            libc::proc_pidfdinfo(
                pid as c_int,
                fd,
                PROC_PIDFDSOCKETINFO,
                (&mut info as *mut SocketFdInfo).cast(),
                size,
            )
        };
        (written == size).then_some(info)
    }

    pub(super) fn listen_addresses(vflag: u8, laddr: [u8; 16], lport: i32) -> Vec<SocketAddr> {
        let port = u16::from_be(lport as u16);
        let ip: IpAddr = if vflag & INI_IPV4 != 0 {
            Ipv4Addr::new(laddr[12], laddr[13], laddr[14], laddr[15]).into()
        } else {
            Ipv6Addr::from(laddr).into()
        };
        super::loopback_addresses(ip, port)
    }

    fn bsd_info(pid: u32) -> Option<(u32, u64)> {
        // SAFETY: plain integers and byte arrays.
        let mut info: libc::proc_bsdinfo = unsafe { zeroed() };
        let size = size_of::<libc::proc_bsdinfo>() as c_int;
        // SAFETY: the buffer is exactly `size` bytes.
        let written = unsafe {
            libc::proc_pidinfo(
                pid as c_int,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        (written == size).then(|| {
            (
                info.pbi_ppid,
                info.pbi_start_tvsec * 1000 + info.pbi_start_tvusec / 1000,
            )
        })
    }

    pub(super) fn started_at(pid: u32) -> Option<u64> {
        bsd_info(pid).map(|(_, started_at)| started_at)
    }

    fn cwd(pid: u32) -> Option<PathBuf> {
        use std::os::unix::ffi::OsStrExt;
        // SAFETY: plain integers and byte arrays.
        let mut info: libc::proc_vnodepathinfo = unsafe { zeroed() };
        let size = size_of::<libc::proc_vnodepathinfo>() as c_int;
        // SAFETY: the buffer is exactly `size` bytes.
        let written = unsafe {
            libc::proc_pidinfo(
                pid as c_int,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                (&mut info as *mut libc::proc_vnodepathinfo).cast(),
                size,
            )
        };
        if written != size {
            return None;
        }
        // SAFETY: `vip_path` is MAXPATHLEN contiguous bytes (libc spells it
        // as 32 rows of 32).
        let bytes: &[u8; 1024] = unsafe { &*info.pvi_cdir.vip_path.as_ptr().cast() };
        let path = CStr::from_bytes_until_nul(bytes).ok()?.to_bytes();
        (!path.is_empty()).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(path)))
    }

    fn arguments(pid: u32) -> Option<Vec<String>> {
        static SPACE: OnceLock<usize> = OnceLock::new();
        let space = *SPACE.get_or_init(|| {
            let mut value: c_int = 0;
            let mut size = size_of::<c_int>();
            let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
            // SAFETY: `value` is an int-sized out parameter.
            let status = unsafe {
                libc::sysctl(
                    mib.as_mut_ptr(),
                    2,
                    (&mut value as *mut c_int).cast(),
                    &mut size,
                    std::ptr::null_mut(),
                    0,
                )
            };
            usize::try_from(value)
                .ok()
                .filter(|v| status == 0 && *v > 0)
                .unwrap_or(1 << 20)
        });
        let mut buffer = vec![0u8; space];
        let mut size = space;
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as c_int];
        // SAFETY: `buffer` holds `size` bytes; the kernel updates `size`.
        let status = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                buffer.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if status != 0 {
            return None;
        }
        buffer.truncate(size);
        super::parse_procargs(&buffer)
    }
}

/// Process start in ms since the epoch, derived the same way for the scanner
/// and for [`same_process`] so the two agree exactly.
#[cfg(target_os = "linux")]
fn linux_clock() -> (u64, u64) {
    let boot = std::fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|line| {
                line.strip_prefix("btime ")
                    .and_then(|v| v.parse::<u64>().ok())
            })
        })
        .unwrap_or(0);
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    (boot, ticks)
}
#[cfg(target_os = "linux")]
fn linux_started_at(stat_fields: &[&str], boot: u64, ticks: u64) -> u64 {
    boot * 1000
        + stat_fields
            .get(19)
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            * 1000
            / ticks
}

#[cfg(target_os = "linux")]
mod platform {
    //! procfs. There is no per-process child list that covers every thread
    //! cheaply, so a pass reads each process's `stat` once for the tree; only
    //! descendants are then opened further (cwd, cmdline, fds).
    use super::{ProcessTable, linux_clock, linux_started_at, proc_address};
    use std::{
        collections::HashMap, fs, net::SocketAddr, os::unix::fs::MetadataExt, path::PathBuf,
    };

    pub(crate) struct Table {
        processes: Option<HashMap<u32, (u32, u64)>>,
        children: HashMap<u32, Vec<u32>>,
        sockets: Option<HashMap<String, SocketAddr>>,
        clock: (u64, u64),
        uid: u32,
    }

    fn stat(pid: u32, (boot, ticks): (u64, u64)) -> Option<(u32, u64)> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let (_, tail) = stat.rsplit_once(") ")?;
        let fields: Vec<_> = tail.split_whitespace().collect();
        let parent = fields.get(1)?.parse().ok()?;
        Some((parent, linux_started_at(&fields, boot, ticks)))
    }

    pub(super) fn started_at(pid: u32) -> Option<u64> {
        stat(pid, linux_clock()).map(|(_, started_at)| started_at)
    }

    impl Table {
        pub(crate) fn new() -> Self {
            Self {
                processes: None,
                children: HashMap::new(),
                sockets: None,
                clock: linux_clock(),
                // SAFETY: geteuid cannot fail.
                uid: unsafe { libc::geteuid() },
            }
        }

        fn processes(&mut self) -> &HashMap<u32, (u32, u64)> {
            if self.processes.is_none() {
                let mut processes = HashMap::new();
                for entry in fs::read_dir("/proc").into_iter().flatten().flatten() {
                    let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                        continue;
                    };
                    if let Some((parent, started_at)) = stat(pid, self.clock) {
                        processes.insert(pid, (parent, started_at));
                        self.children.entry(parent).or_default().push(pid);
                    }
                }
                self.processes = Some(processes);
            }
            self.processes.as_ref().unwrap()
        }

        fn sockets(&mut self) -> &HashMap<String, SocketAddr> {
            self.sockets.get_or_insert_with(|| {
                let mut sockets = HashMap::new();
                for (file, ipv6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
                    if let Ok(text) = fs::read_to_string(file) {
                        for line in text.lines().skip(1) {
                            let parts: Vec<_> = line.split_whitespace().collect();
                            if parts.len() <= 9 || parts[3] != "0A" {
                                continue;
                            }
                            if let Some(address) = proc_address(parts[1], ipv6) {
                                sockets.insert(parts[9].to_string(), address);
                            }
                        }
                    }
                }
                sockets
            })
        }
    }

    impl ProcessTable for Table {
        fn children(&mut self, pid: u32) -> Vec<u32> {
            self.processes();
            self.children.get(&pid).cloned().unwrap_or_default()
        }

        fn info(&mut self, pid: u32) -> Option<(u32, u64)> {
            self.processes().get(&pid).copied()
        }

        fn listening(&mut self, pid: u32) -> Vec<SocketAddr> {
            let path = PathBuf::from(format!("/proc/{pid}"));
            if !fs::metadata(&path).is_ok_and(|m| m.uid() == self.uid) {
                return Vec::new();
            }
            let Ok(fds) = fs::read_dir(path.join("fd")) else {
                return Vec::new();
            };
            let inodes: Vec<String> = fds
                .flatten()
                .filter_map(|fd| {
                    let link = fs::read_link(fd.path()).ok()?;
                    let link = link.to_string_lossy();
                    Some(link.strip_prefix("socket:[")?.strip_suffix(']')?.to_owned())
                })
                .collect();
            if inodes.is_empty() {
                return Vec::new();
            }
            let sockets = self.sockets();
            inodes
                .iter()
                .filter_map(|inode| sockets.get(inode).copied())
                .collect()
        }

        fn details(&mut self, pid: u32) -> Option<(PathBuf, Vec<String>)> {
            let path = PathBuf::from(format!("/proc/{pid}"));
            let cwd = fs::read_link(path.join("cwd")).ok()?;
            let args = fs::read(path.join("cmdline"))
                .unwrap_or_default()
                .split(|b| *b == 0)
                .filter(|b| !b.is_empty())
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .collect();
            Some((cwd, args))
        }
    }
}

#[cfg(target_os = "linux")]
fn proc_address(value: &str, ipv6: bool) -> Option<SocketAddr> {
    let (host, port) = value.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let ip = if ipv6 {
        if host.len() != 32 {
            return None;
        }
        let mut bytes = [0; 16];
        for (i, chunk) in host.as_bytes().chunks_exact(8).enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(
                &u32::from_str_radix(std::str::from_utf8(chunk).ok()?, 16)
                    .ok()?
                    .to_le_bytes(),
            );
        }
        IpAddr::V6(Ipv6Addr::from(bytes))
    } else {
        IpAddr::V4(Ipv4Addr::from(
            u32::from_str_radix(host, 16).ok()?.to_le_bytes(),
        ))
    };
    if !ip.is_loopback() && !ip.is_unspecified() {
        return None;
    }
    Some(SocketAddr::new(
        if ip.is_unspecified() {
            if ipv6 {
                Ipv6Addr::LOCALHOST.into()
            } else {
                Ipv4Addr::LOCALHOST.into()
            }
        } else {
            ip
        },
        port,
    ))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    use super::ProcessTable;
    use std::{net::SocketAddr, path::PathBuf};

    pub(crate) struct Table;
    impl Table {
        pub(crate) fn new() -> Self {
            Self
        }
    }
    impl ProcessTable for Table {
        fn children(&mut self, _pid: u32) -> Vec<u32> {
            Vec::new()
        }
        fn info(&mut self, _pid: u32) -> Option<(u32, u64)> {
            None
        }
        fn listening(&mut self, _pid: u32) -> Vec<SocketAddr> {
            Vec::new()
        }
        fn details(&mut self, _pid: u32) -> Option<(PathBuf, Vec<String>)> {
            None
        }
    }
    pub(super) fn started_at(_pid: u32) -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authentication_commands_are_not_preview_candidates() {
        let listener = |args: &[&str]| Listener {
            pid: 1,
            parent: 0,
            cwd: "/project".into(),
            args: args.iter().map(|arg| (*arg).into()).collect(),
            started_at: 1,
            address: "127.0.0.1:3000".parse().unwrap(),
            zeron_owned: true,
        };
        for args in [
            vec!["/usr/bin/infisical", "login"],
            vec![
                "infisical",
                "--silent",
                "login",
                "--domain",
                "https://app.infisical.com",
            ],
            vec!["gh", "auth", "login"],
            vec!["aws", "sso", "login"],
            vec!["gcloud", "auth", "application-default", "login"],
            vec!["codex", "login"],
            vec!["node", "/tools/firebase.js", "login"],
            vec!["cli", "authenticate"],
            vec!["cli", "signin"],
            vec!["cli", "sign-in"],
            vec!["cli", "oauth"],
            vec!["cli", "oauth2"],
        ] {
            assert!(listener(&args).is_authentication_command(), "{args:?}");
        }
        for args in [
            vec![],
            vec!["node", "/project/auth/server.js"],
            vec!["node", "/project/login.js"],
            vec!["node", "/project/node_modules/vite/bin/vite.js"],
            vec!["next", "dev"],
            vec!["python3", "-m", "http.server", "3000"],
            vec!["python3", "-c", "import auth; auth.serve()"],
            vec!["server", "--auth", "--login-url=https://example.com/login"],
            vec!["infisical", "run", "--", "node", "server.js", "login"],
        ] {
            assert!(!listener(&args).is_authentication_command(), "{args:?}");
        }
    }
    #[test]
    fn service_identity_ignores_port_configuration() {
        let args = |port: &str| vec!["node".into(), "api.js".into(), "--port".into(), port.into()];
        assert_eq!(
            command_identity(&args("3000")),
            command_identity(&args("3001"))
        );
        assert_ne!(
            command_identity(&args("3000")),
            command_identity(&["node".into(), "docs.js".into()])
        );
    }

    /// An in-memory process table: pid → (parent, start, listening ports).
    #[derive(Default)]
    struct Fake {
        processes: HashMap<u32, (u32, u64, Vec<u16>)>,
        queried: Vec<u32>,
    }
    impl Fake {
        fn spawn(&mut self, pid: u32, parent: u32, started_at: u64, ports: &[u16]) {
            self.processes
                .insert(pid, (parent, started_at, ports.to_vec()));
        }
    }
    impl ProcessTable for Fake {
        fn children(&mut self, pid: u32) -> Vec<u32> {
            let mut children: Vec<_> = self
                .processes
                .iter()
                .filter(|(_, (parent, _, _))| *parent == pid)
                .map(|(pid, _)| *pid)
                .collect();
            children.sort();
            children
        }
        fn info(&mut self, pid: u32) -> Option<(u32, u64)> {
            self.processes.get(&pid).map(|(p, s, _)| (*p, *s))
        }
        fn listening(&mut self, pid: u32) -> Vec<SocketAddr> {
            self.queried.push(pid);
            self.processes
                .get(&pid)
                .map(|(_, _, ports)| {
                    ports
                        .iter()
                        .map(|port| SocketAddr::from(([127, 0, 0, 1], *port)))
                        .collect()
                })
                .unwrap_or_default()
        }
        fn details(&mut self, pid: u32) -> Option<(PathBuf, Vec<String>)> {
            self.processes
                .contains_key(&pid)
                .then(|| ("/work/app".into(), vec![format!("server-{pid}")]))
        }
    }

    #[test]
    fn only_descendants_of_the_root_are_examined() {
        let mut table = Fake::default();
        table.spawn(100, 1, 10, &[]); // the engine
        table.spawn(200, 100, 20, &[]); // agent harness
        table.spawn(201, 200, 21, &[]); // its shell
        table.spawn(202, 201, 22, &[5173]); // the dev server it started
        table.spawn(300, 100, 30, &[]); // a terminal
        table.spawn(400, 1, 40, &[3000]); // the user's own server elsewhere
        table.spawn(401, 400, 41, &[3001]);
        let mut descendants = Descendants::default();
        let scan = descendants.scan_with(&mut table, 100);
        assert!(scan.live);
        assert_eq!(descendants.pids(), [200, 201, 202, 300].into());
        let ports: Vec<_> = scan.listeners.iter().map(|l| l.address.port()).collect();
        assert_eq!(ports, [5173]);
        assert_eq!(scan.listeners[0].parent, 201);
        assert_eq!(scan.listeners[0].started_at, 22);
        assert!(scan.listeners[0].zeron_owned);
        table.queried.sort();
        assert_eq!(
            table.queried,
            [200, 201, 202, 300],
            "no other process is opened"
        );
    }

    #[test]
    fn a_server_outliving_its_agent_stays_in_scope_until_it_exits() {
        let mut table = Fake::default();
        table.spawn(200, 100, 20, &[]);
        table.spawn(202, 200, 22, &[5173]);
        let mut descendants = Descendants::default();
        assert_eq!(descendants.scan_with(&mut table, 100).listeners.len(), 1);
        // The harness exits; launchd adopts the dev server.
        table.processes.remove(&200);
        table.spawn(202, 1, 22, &[5173]);
        let scan = descendants.scan_with(&mut table, 100);
        assert!(scan.live);
        assert_eq!(scan.listeners.len(), 1, "the orphaned server is still ours");
        // Children it starts later are ours too.
        table.spawn(203, 202, 23, &[5174]);
        let scan = descendants.scan_with(&mut table, 100);
        assert_eq!(scan.listeners.len(), 2);
        // The pid is reused by an unrelated process: not ours.
        table.processes.clear();
        table.spawn(202, 1, 99, &[8080]);
        let scan = descendants.scan_with(&mut table, 100);
        assert!(scan.listeners.is_empty());
        assert!(!scan.live, "nothing below the engine is left alive");
        assert!(descendants.pids().is_empty());
    }

    #[test]
    fn processes_exiting_mid_pass_are_skipped() {
        struct Vanishing(Fake);
        impl ProcessTable for Vanishing {
            fn children(&mut self, pid: u32) -> Vec<u32> {
                self.0.children(pid)
            }
            fn info(&mut self, pid: u32) -> Option<(u32, u64)> {
                self.0.info(pid)
            }
            fn listening(&mut self, pid: u32) -> Vec<SocketAddr> {
                self.0.listening(pid)
            }
            // Exits between listing its sockets and reading its cwd.
            fn details(&mut self, _pid: u32) -> Option<(PathBuf, Vec<String>)> {
                None
            }
        }
        let mut table = Fake::default();
        table.spawn(200, 100, 20, &[5173]);
        table.spawn(201, 100, 21, &[]);
        let mut descendants = Descendants::default();
        let scan = descendants.scan_with(&mut Vanishing(table), 100);
        assert!(scan.listeners.is_empty());
        assert!(scan.live);
    }

    #[test]
    fn procargs_yield_the_arguments_ps_printed() {
        let mut buffer = 3i32.to_ne_bytes().to_vec();
        buffer.extend_from_slice(b"/usr/local/bin/node\0\0\0\0");
        buffer.extend_from_slice(b"node\0server.js\0--name=my app\0");
        buffer.extend_from_slice(b"PATH=/usr/bin\0SECRET=1\0");
        assert_eq!(
            parse_procargs(&buffer).unwrap(),
            ["node", "server.js", "--name=my", "app"]
        );
        assert!(parse_procargs(&[1, 0]).is_none());
        let mut empty = 0i32.to_ne_bytes().to_vec();
        empty.extend_from_slice(b"/bin/x\0");
        assert_eq!(parse_procargs(&empty).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn wildcard_and_loopback_binds_are_reachable_others_are_not() {
        let v4 = SocketAddr::from(([127, 0, 0, 1], 5173));
        let v6 = SocketAddr::from((Ipv6Addr::LOCALHOST, 5173));
        assert_eq!(
            loopback_addresses(Ipv4Addr::UNSPECIFIED.into(), 5173),
            [v4, v6]
        );
        assert_eq!(
            loopback_addresses(Ipv6Addr::UNSPECIFIED.into(), 5173),
            [v4, v6]
        );
        assert_eq!(loopback_addresses(Ipv4Addr::LOCALHOST.into(), 5173), [v4]);
        assert_eq!(loopback_addresses(Ipv6Addr::LOCALHOST.into(), 5173), [v6]);
        let mapped: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert_eq!(loopback_addresses(mapped, 5173), [v4]);
        assert!(loopback_addresses([192, 168, 1, 2].into(), 5173).is_empty());
        assert!(loopback_addresses(Ipv4Addr::LOCALHOST.into(), 0).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn libproc_mirrors_match_the_sdk_layout() {
        use platform::*;
        use std::mem::{offset_of, size_of};
        // `sizeof`/`offsetof` from <sys/proc_info.h> (macOS SDK 26).
        assert_eq!(size_of::<SocketFdInfo>(), 792);
        assert_eq!(size_of::<SocketInfo>(), 768);
        assert_eq!(size_of::<InSockInfo>(), 80);
        assert_eq!(size_of::<TcpSockInfo>(), 120);
        assert_eq!(offset_of!(SocketFdInfo, psi), 24);
        assert_eq!(offset_of!(SocketInfo, family), 160);
        assert_eq!(offset_of!(SocketInfo, kind), 232);
        assert_eq!(offset_of!(SocketInfo, proto), 240);
        assert_eq!(offset_of!(InSockInfo, lport), 4);
        assert_eq!(offset_of!(InSockInfo, vflag), 24);
        assert_eq!(offset_of!(InSockInfo, laddr), 48);
        assert_eq!(offset_of!(TcpSockInfo, state), 80);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn libproc_listener_fields_decode() {
        let mut laddr = [0; 16];
        laddr[12..].copy_from_slice(&[127, 0, 0, 1]);
        let port = i32::from(5173u16.to_be());
        assert_eq!(
            platform::listen_addresses(1, laddr, port),
            [SocketAddr::from(([127, 0, 0, 1], 5173))]
        );
        assert_eq!(
            platform::listen_addresses(2, Ipv6Addr::LOCALHOST.octets(), port),
            [SocketAddr::from((Ipv6Addr::LOCALHOST, 5173))]
        );
        assert_eq!(platform::listen_addresses(3, [0; 16], port).len(), 2);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn the_os_table_reports_this_process_listeners_only() {
        let v4 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let v6 = std::net::TcpListener::bind("[::1]:0").ok();
        let any = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        // An established connection is not a listener.
        let client = std::net::TcpStream::connect(v4.local_addr().unwrap()).unwrap();
        let _accepted = v4.accept().unwrap();
        let found = platform::Table::new().listening(std::process::id());
        assert!(found.contains(&v4.local_addr().unwrap()), "{found:?}");
        if let Some(v6) = &v6 {
            assert!(found.contains(&v6.local_addr().unwrap()), "{found:?}");
        }
        let port = any.local_addr().unwrap().port();
        assert!(found.contains(&SocketAddr::from(([127, 0, 0, 1], port))));
        let ephemeral = client.local_addr().unwrap().port();
        assert!(!found.iter().any(|a| a.port() == ephemeral), "{found:?}");
    }

    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn real_descendants_are_found_including_orphans() {
        let temp = tempfile::tempdir().unwrap();
        let pidfile = temp.path().join("server.pid");
        // A shell starts a listening server in the background and exits, as
        // an agent's `npm run dev &` does; the server is reparented.
        let mut shell = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "python3 -c 'import socket,time; s=socket.socket(); s.bind((\"127.0.0.1\",0)); s.listen(); time.sleep(30)' & echo $! > {}; sleep 1",
                pidfile.display()
            ))
            .current_dir(temp.path())
            .spawn()
            .unwrap();
        let unrelated = Kill(
            std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .unwrap(),
        );
        let server = loop {
            if let Some(pid) = std::fs::read_to_string(&pidfile)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
            {
                break pid;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        struct KillPid(u32);
        impl Drop for KillPid {
            fn drop(&mut self) {
                unsafe { libc::kill(self.0 as i32, libc::SIGKILL) };
            }
        }
        let _server = KillPid(server);
        let mut descendants = Descendants::default();
        // Rooted at the shell: the sibling `sleep` is not below it.
        let root = shell.id();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let first = loop {
            let scan = descendants.scan(root);
            if !scan.listeners.is_empty() || std::time::Instant::now() > deadline {
                break scan;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(first.listeners.iter().all(|l| l.pid == server), "{first:?}");
        assert!(
            !first.listeners.is_empty(),
            "server not found below its shell"
        );
        assert!(!descendants.pids().contains(&unrelated.0.id()));
        assert!(!descendants.pids().contains(&1));
        let listener = &first.listeners[0];
        assert_eq!(listener.cwd, temp.path().canonicalize().unwrap());
        assert!(
            listener.args.iter().any(|a| a.contains("socket,time")),
            "{:?}",
            listener.args
        );
        assert!(same_process(server, listener.started_at));
        shell.wait().unwrap();
        let orphaned = descendants.scan(root);
        assert!(
            orphaned.listeners.iter().any(|l| l.pid == server),
            "the reparented server stays discovered"
        );
        drop(_server);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let scan = descendants.scan(root);
            if !scan.live {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{scan:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn accepts_only_loopback_reachable_listeners() {
        assert_eq!(
            proc_address("0100007F:1435", false).unwrap(),
            "127.0.0.1:5173".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            proc_address("00000000000000000000000001000000:1435", true).unwrap(),
            "[::1]:5173".parse::<SocketAddr>().unwrap()
        );
        assert!(proc_address("0101A8C0:1435", false).is_none());
        assert!(proc_address("malformed:00", true).is_none());
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn same_process_matches_the_scanner_start_time() {
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        let guard = Kill(child);
        // The scanner's start time for this child, through its own table.
        let mut table = platform::Table::new();
        let (parent, started_at) = table.info(pid).unwrap();
        assert_eq!(parent, std::process::id());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!(started_at > now - 60_000 && started_at <= now + 1_000);
        assert!(same_process(pid, started_at));
        assert!(!same_process(pid, started_at + 1000));
        assert!(!same_process(pid, 0));
        drop(guard);
        assert!(
            !same_process(pid, started_at),
            "a reaped pid no longer matches"
        );
    }
    #[tokio::test]
    async fn requires_an_http_response() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            for response in [
                b"SSH-2.0-test\r\n".as_slice(),
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n",
            ] {
                let (mut client, _) = listener.accept().await.unwrap();
                let mut input = [0; 512];
                let _ = client.read(&mut input).await;
                client.write_all(response).await.unwrap();
            }
        });
        assert!(!is_http(address).await);
        assert!(is_http(address).await);
        task.await.unwrap();
    }
}
